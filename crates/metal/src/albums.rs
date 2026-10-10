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
            albums.into_iter().flat_map(|albums| {
                albums
                    .values()
                    .map(|album| (album.title.as_str(), album.count))
            }),
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
