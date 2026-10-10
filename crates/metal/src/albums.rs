use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DAY, normalize, now, number};

/// An account-specific album count; ambiguous matches have no count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumPlays {
    pub count: Option<u64>,
    pub fetched_at: u64,
}

/// A complete overall listening chart, indexed by artist and album rather than provider IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlbumLibrary {
    pub artists: HashMap<String, HashMap<String, ListenedAlbum>>,
    pub fetched_at: u64,
    /// Unresolved metadata is retried without discarding other albums' daily updates.
    #[serde(default)]
    pub(super) pending: Vec<(String, String)>,
}

/// Preserves a source spelling; conflicting canonical chart records have an unknown count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListenedAlbum {
    pub title: String,
    pub count: Option<u64>,
}

impl AlbumLibrary {
    /// Only complete libraries infer zero; similarly plausible titles remain unknown.
    pub fn plays(&self, artist: &str, title: &str) -> AlbumPlays {
        let albums = self.artists.get(&normalize(artist));
        if let Some(album) = albums.and_then(|albums| albums.get(&title_key(title))) {
            return AlbumPlays {
                count: album.count,
                fetched_at: self.fetched_at,
            };
        }
        let count = match match_album_title(
            title,
            albums
                .into_iter()
                .flat_map(|albums| {
                    albums
                        .values()
                        .map(|album| (album.title.as_str(), album.count))
                })
                .chain(
                    self.pending
                        .iter()
                        .filter(|(pending_artist, pending_title)| {
                            normalize(pending_artist) == normalize(artist)
                                && albums.is_none_or(|albums| {
                                    !albums.contains_key(&title_key(pending_title))
                                })
                        })
                        .map(|(_, title)| (title.as_str(), None)),
                ),
        ) {
            AlbumMatch::Matched(count) => count,
            AlbumMatch::Missing => Some(0),
            AlbumMatch::Ambiguous => None,
        };
        AlbumPlays {
            count,
            fetched_at: self.fetched_at,
        }
    }

    /// Artist visits never extend the original library's 24-hour freshness window.
    pub fn expired(&self) -> bool {
        now().saturating_sub(self.fetched_at) >= DAY
    }

    /// Canonical collisions stay unknown instead of choosing or adding different chart records.
    pub(super) fn insert(&mut self, artist: &str, title: String, count: u64) {
        self.artists
            .entry(normalize(artist))
            .or_default()
            .entry(title_key(&title))
            .and_modify(|kept| kept.count = None)
            .or_insert(ListenedAlbum {
                title,
                count: Some(count),
            });
    }

    /// Replaces a personal total; valid but unresolved metadata returns false for later retry.
    pub(super) fn refresh_album(
        &mut self,
        artist: &str,
        title: &str,
        value: &Value,
    ) -> Result<bool> {
        let album = &value["album"];
        if !album.is_object() {
            bail!("invalid Last.fm album metadata");
        }
        let returned_artist = album["artist"]
            .as_str()
            .context("Last.fm album metadata has no artist")?;
        let returned_title = album["name"]
            .as_str()
            .context("Last.fm album metadata has no title")?;
        if normalize(returned_artist) != normalize(artist)
            || title_key(returned_title) != title_key(title)
        {
            return Ok(false);
        }
        let Some(count) = album.get("userplaycount").and_then(number) else {
            return Ok(false);
        };
        // Pending spellings can also collide with a newly resolved canonical identity.
        let ambiguous = self.pending.iter().any(|(pending_artist, pending_title)| {
            normalize(pending_artist) == normalize(artist)
                && title_key(pending_title) == title_key(title)
                && normalize(pending_title) != normalize(title)
        });
        let kept = self
            .artists
            .entry(normalize(artist))
            .or_default()
            .entry(title_key(title))
            .or_insert_with(|| ListenedAlbum {
                title: title.to_owned(),
                count: Some(count),
            });
        if kept.count.is_some() {
            kept.count = match !ambiguous && normalize(&kept.title) == normalize(title) {
                true => Some(count),
                false => None,
            };
        }
        self.pending.retain(|(pending_artist, pending_title)| {
            normalize(pending_artist) != normalize(artist)
                || normalize(pending_title) != normalize(title)
        });
        Ok(true)
    }

    /// Preserves known counts and records unresolved new albums as unknown rather than zero.
    pub(super) fn defer_album(&mut self, artist: &str, title: &str) {
        if let Some(kept) = self
            .artists
            .get_mut(&normalize(artist))
            .and_then(|albums| albums.get_mut(&title_key(title)))
            && normalize(&kept.title) != normalize(title)
        {
            kept.count = None;
        }
        if !self.pending.iter().any(|(kept_artist, kept_title)| {
            normalize(kept_artist) == normalize(artist) && normalize(kept_title) == normalize(title)
        }) {
            self.pending.push((artist.to_owned(), title.to_owned()));
        }
    }
}

/// Distinguishes no candidate from an unsafe tie, so a complete chart can infer zero correctly.
pub enum AlbumMatch<T> {
    Matched(T),
    Missing,
    Ambiguous,
}

/// Prefers exact names, then a unique high-confidence fuzzy match with a runner-up margin.
/// The caller must already have restricted candidates to the requested artist.
pub fn match_album_title<'a, T>(
    title: &str,
    candidates: impl IntoIterator<Item = (&'a str, T)>,
) -> AlbumMatch<T> {
    let mut best: Option<(u32, String, T)> = None;
    let mut runner_up = 0;
    for (name, value) in candidates {
        let Some(score) = album_title_score(title, name) else {
            continue;
        };
        let key = title_key(name);
        if let Some((_, kept, _)) = &best
            && *kept == key
        {
            runner_up = best.as_ref().map(|(score, _, _)| *score).unwrap_or(0);
            continue;
        }
        if best.as_ref().is_none_or(|(kept, _, _)| score > *kept) {
            runner_up = best.as_ref().map(|(score, _, _)| *score).unwrap_or(0);
            best = Some((score, key, value));
        } else {
            runner_up = runner_up.max(score);
        }
    }
    match best {
        Some((1000, _, value)) if runner_up < 1000 => AlbumMatch::Matched(value),
        Some((score, _, value)) if score < 1000 && score.saturating_sub(runner_up) >= 50 => {
            AlbumMatch::Matched(value)
        }
        Some(_) => AlbumMatch::Ambiguous,
        None => AlbumMatch::Missing,
    }
}

/// Offers a shorter search query for any trailing parenthetical annotation, preserving display.
pub fn album_search_title(title: &str) -> &str {
    title
        .trim()
        .strip_suffix(')')
        .and_then(|title| title.rsplit_once(" ("))
        .filter(|(base, suffix)| !base.trim().is_empty() && !suffix.contains(['(', ')']))
        .map(|(base, _)| base.trim_end())
        .unwrap_or(title)
}

/// One validated API page; incomplete charts never become an AlbumLibrary snapshot.
pub(super) struct AlbumPage {
    pub albums: Vec<(String, String, u64)>,
    pub pages: u64,
    pub total: u64,
}

impl AlbumPage {
    /// Verifies account and pagination before accepting the user endpoint's personal playcounts.
    pub fn parse(user: &str, page: u64, value: &Value) -> Result<Self> {
        let data = &value["topalbums"];
        let attr = &data["@attr"];
        let owner = attr["user"]
            .as_str()
            .context("Last.fm album chart has no user")?;
        if owner.trim().to_lowercase() != user.trim().to_lowercase()
            || number(&attr["page"]) != Some(page)
        {
            bail!("Last.fm album chart account or page mismatch");
        }
        let pages = number(&attr["totalPages"]).context("Last.fm album chart has no page count")?;
        let total = number(&attr["total"]).context("Last.fm album chart has no total")?;
        if pages > 1000 || (total > 0 && pages == 0) {
            bail!("invalid Last.fm album chart pagination");
        }
        let rows = data["album"]
            .as_array()
            .context("invalid Last.fm album chart")?;
        let mut albums = Vec::with_capacity(rows.len());
        for row in rows {
            let artist = row["artist"]["name"]
                .as_str()
                .context("Last.fm album has no artist")?;
            let title = row["name"].as_str().context("Last.fm album has no title")?;
            let count = number(&row["playcount"]).context("Last.fm album has no personal plays")?;
            if artist.trim().is_empty() || title.trim().is_empty() {
                bail!("Last.fm album chart has an empty identity");
            }
            albums.push((artist.to_owned(), title.to_owned(), count));
        }
        Ok(Self {
            albums,
            pages,
            total,
        })
    }
}

/// One bounded recent-scrobble page; now-playing rows never identify completed album listens.
pub(super) struct RecentAlbumPage {
    pub albums: Vec<(String, String)>,
    pub scrobbles: u64,
    pub pages: u64,
    pub total: u64,
}

impl RecentAlbumPage {
    /// Checks the account and pagination, retaining only completed plays in the requested range.
    pub fn parse(user: &str, page: u64, from: u64, to: u64, value: &Value) -> Result<Self> {
        let data = &value["recenttracks"];
        let attr = &data["@attr"];
        let owner = attr["user"]
            .as_str()
            .context("Last.fm recent tracks have no user")?;
        if owner.trim().to_lowercase() != user.trim().to_lowercase()
            || number(&attr["page"]) != Some(page)
        {
            bail!("Last.fm recent tracks account or page mismatch");
        }
        let pages =
            number(&attr["totalPages"]).context("Last.fm recent tracks have no page count")?;
        let total = number(&attr["total"]).context("Last.fm recent tracks have no total")?;
        if pages > 1000 || (total > 0 && pages == 0) {
            bail!("invalid Last.fm recent tracks pagination");
        }
        let rows = data["track"]
            .as_array()
            .context("invalid Last.fm recent tracks")?;
        let mut albums = Vec::new();
        let mut scrobbles = 0;
        for row in rows {
            if row["@attr"]["nowplaying"].as_str() == Some("true") {
                continue;
            }
            let at = number(&row["date"]["uts"]).context("Last.fm scrobble has no timestamp")?;
            scrobbles += 1;
            if at < from || at > to {
                continue;
            }
            let artist = row["artist"]["#text"]
                .as_str()
                .context("Last.fm scrobble has no artist")?;
            let title = row["album"]["#text"]
                .as_str()
                .context("Last.fm scrobble has no album field")?;
            if !artist.trim().is_empty() && !title.trim().is_empty() {
                albums.push((artist.to_owned(), title.to_owned()));
            }
        }
        Ok(Self {
            albums,
            scrobbles,
            pages,
            total,
        })
    }
}

/// Treats typography as equivalent while retaining word boundaries and volume numbers.
fn title_key(title: &str) -> String {
    normalize(title)
        .chars()
        .map(|character| match character.is_alphanumeric() {
            true => character,
            false => ' ',
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scores generic annotations and minor typos without merging numbered or distinct recordings.
pub fn album_title_score(left: &str, right: &str) -> Option<u32> {
    let left_key = title_key(left);
    let right_key = title_key(right);
    if left_key == right_key {
        return (!left_key.is_empty()).then_some(1000);
    }
    let variants = |key: &str| {
        key.split_whitespace()
            .filter(|word| matches!(*word, "live" | "acoustic" | "instrumental" | "remix"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    if variants(&left_key) != variants(&right_key) {
        return None;
    }
    let left_core = title_key(album_search_title(left));
    let right_core = title_key(album_search_title(right));
    let volumes = |key: &str| {
        key.split_whitespace()
            .filter(|word| {
                word.parse::<u64>().is_ok()
                    || matches!(
                        *word,
                        "i" | "ii" | "iii" | "iv" | "v" | "vi" | "vii" | "viii" | "ix" | "x"
                    )
            })
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let left_volumes = volumes(&left_key);
    let right_volumes = volumes(&right_key);
    if !left_volumes.is_empty() && !right_volumes.is_empty() && left_volumes != right_volumes {
        return None;
    }
    if left_core.len().min(right_core.len()) < 6 || volumes(&left_core) != volumes(&right_core) {
        return None;
    }
    if left_core == right_core {
        return Some(950);
    }
    let score = strsim::normalized_levenshtein(&left_key, &right_key);
    (score >= 0.90).then_some((score * 1000.0) as u32)
}
