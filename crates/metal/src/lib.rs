mod parse;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const DAY: u64 = 86_400;
const MA: &str = "https://www.metal-archives.com";
const POLICY_KEY: &str = "metal:policy:v1";

/// A Metal Archives band candidate; namesakes remain separate by ID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Band {
    pub id: String,
    pub name: String,
    pub url: String,
    pub genre: String,
    pub country: String,
}

/// A release with review aggregates, without fetching individual reviews.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub id: String,
    pub band: Band,
    pub title: String,
    pub kind: String,
    pub date: String,
    pub url: String,
    pub review_score: Option<f64>,
    pub review_count: u32,
}

/// A complete remote listing or an explicitly imported, potentially partial saved response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseFeed {
    pub releases: Vec<Release>,
    pub total: usize,
    pub imported: bool,
}

/// Public Last.fm artist information or a user's library play count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtistStats {
    pub name: String,
    pub playcount: u64,
    pub listeners: Option<u64>,
    pub tags: Vec<String>,
    pub url: String,
}

/// User-scoped listening history indexed by normalized artist name.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub artists: HashMap<String, ArtistStats>,
    pub updated_at: u64,
    pub source: String,
}

/// Case-folds and transliterates names while retaining word boundaries and symbols.
pub fn normalize(name: &str) -> String {
    deunicode::deunicode(name)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Serialize, Deserialize)]
struct Snapshot<T> {
    at: u64,
    value: T,
}

#[derive(Default, Serialize, Deserialize)]
struct Policy {
    day: u64,
    requests: u32,
    next_request: u64,
    blocked_until: u64,
}

/// Retains Last.fm's error code without exposing API keys or request URLs.
#[derive(Debug)]
struct LastfmError(u64);

/// UI-independent metadata client. Clones share operation coalescing and request pacing.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    cache: storage::Cache,
    operation: Arc<Mutex<()>>,
}

impl Client {
    pub fn new(cache: storage::Cache) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("Sonora-Metal/0.1 (+https://github.com/pelletiermaxime/sonora)")
                .build()
                .expect("valid metadata HTTP configuration"),
            cache,
            operation: Arc::new(Mutex::new(())),
        }
    }

    fn snapshot<T: DeserializeOwned>(&self, key: &str) -> Result<Option<Snapshot<T>>> {
        self.cache
            .read(key)?
            .map(|s| serde_json::from_str(&s).context("cannot decode metal cache"))
            .transpose()
    }

    fn save<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let at = now();
        self.cache.write(
            key,
            &serde_json::to_string(&Snapshot { at, value })?,
            at as i64,
        )
    }

    async fn cached<T, F>(&self, key: &str, ttl: u64, fetch: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: Future<Output = Result<T>>,
    {
        let _operation = self.operation.lock().await;
        let cached = self.snapshot::<T>(key)?;
        if cached
            .as_ref()
            .is_some_and(|snapshot| now().saturating_sub(snapshot.at) < ttl)
        {
            return Ok(cached.expect("snapshot exists").value);
        }
        match fetch.await {
            Ok(value) => {
                self.save(key, &value)?;
                Ok(value)
            }
            Err(error) => match cached {
                Some(snapshot) => {
                    log::warn!("metal: refresh failed; using stale cached metadata");
                    Ok(snapshot.value)
                }
                None => Err(error),
            },
        }
    }

    /// Returns every remote page or a saved response explicitly marked as imported and possibly partial.
    pub async fn releases(&self, from: &str, to: &str) -> Result<ReleaseFeed> {
        let start: jiff::civil::Date = from.parse().context("invalid release start date")?;
        let end: jiff::civil::Date = to.parse().context("invalid release end date")?;
        if start > end {
            bail!("release date range is reversed");
        }
        if let Some(saved) =
            self.snapshot::<ReleaseFeed>(&format!("metal:imported-releases:{from}:{to}"))?
        {
            return Ok(saved.value);
        }
        let ttl = if end < jiff::Zoned::now().date() {
            30 * DAY
        } else {
            DAY / 2
        };
        self.cached(&format!("metal:releases:{from}:{to}"), ttl, async {
            let mut releases = Vec::new();
            let mut offset = 0;
            loop {
                let text = self
                    .ma(
                        "/release/ajax-upcoming/json",
                        &[
                            ("fromDate", from.to_owned()),
                            ("toDate", to.to_owned()),
                            ("iDisplayLength", "500".into()),
                            ("iDisplayStart", offset.to_string()),
                        ],
                    )
                    .await?;
                let data: Value =
                    serde_json::from_str(&text).context("cannot decode releases response")?;
                let rows = rows(&data)?;
                let total = number(&data["iTotalDisplayRecords"])
                    .context("release response has no total")?;
                for row in rows {
                    releases.push(parse::upcoming(row)?);
                }
                offset += rows.len();
                if offset as u64 >= total {
                    break;
                }
                if rows.is_empty() {
                    bail!("incomplete release pagination");
                }
            }
            Ok(ReleaseFeed {
                total: releases.len(),
                releases,
                imported: false,
            })
        })
        .await
    }

    /// Imports saved records, preserving the advertised total so partial exports stay visibly partial.
    pub async fn import_releases(&self, from: &str, to: &str, text: &str) -> Result<ReleaseFeed> {
        let _operation = self.operation.lock().await;
        let start: jiff::civil::Date = from.parse().context("invalid release start date")?;
        let end: jiff::civil::Date = to.parse().context("invalid release end date")?;
        if start > end {
            bail!("release date range is reversed");
        }
        let data: Value =
            serde_json::from_str(text).context("cannot decode saved release response")?;
        let rows = rows(&data)?;
        let total =
            number(&data["iTotalDisplayRecords"]).context("saved response has no total")? as usize;
        if rows.len() > total {
            bail!("saved response has more rows than its advertised total");
        }
        let releases = rows
            .iter()
            .map(parse::upcoming)
            .collect::<Result<Vec<_>>>()?;
        for release in &releases {
            let date: jiff::civil::Date = release
                .date
                .parse()
                .context("saved release has an imprecise date")?;
            if date < start || date > end {
                bail!("saved releases do not belong to the selected date range");
            }
        }
        let feed = ReleaseFeed {
            releases,
            total,
            imported: true,
        };
        self.save(&format!("metal:imported-releases:{from}:{to}"), &feed)?;
        Ok(feed)
    }

    /// Searches exact normalized band names and preserves all matching namesakes.
    pub async fn bands(&self, name: &str) -> Result<Vec<Band>> {
        self.cached(
            &format!("metal:bands:{}", normalize(name)),
            30 * DAY,
            async {
                let mut bands = Vec::new();
                let mut offset = 0;
                loop {
                    let text = self
                        .ma(
                            "/search/ajax-band-search",
                            &[
                                ("field", "name".into()),
                                ("query", name.into()),
                                ("iDisplayLength", "500".into()),
                                ("iDisplayStart", offset.to_string()),
                            ],
                        )
                        .await?;
                    let data: Value =
                        serde_json::from_str(&text).context("cannot decode band search")?;
                    let rows = rows(&data)?;
                    for row in rows {
                        let band = parse::band(row)?;
                        if normalize(&band.name) == normalize(name) {
                            bands.push(band);
                        }
                    }
                    offset += rows.len();
                    let total = number(&data["iTotalDisplayRecords"])
                        .context("band search has no total")?;
                    if offset as u64 >= total {
                        break;
                    }
                    if rows.is_empty() {
                        bail!("incomplete band search pagination");
                    }
                }
                for band in &bands {
                    self.save(&format!("metal:band-id:{}", band.id), band)?;
                }
                Ok(bands)
            },
        )
        .await
    }

    pub async fn discography(&self, band: &Band) -> Result<Vec<Release>> {
        if !band.id.chars().all(|c| c.is_ascii_digit()) || band.id.is_empty() {
            bail!("invalid Metal Archives band ID");
        }
        self.cached(&format!("metal:discography:{}", band.id), 7 * DAY, async {
            let text = self
                .ma(&format!("/band/discography/id/{}/tab/all", band.id), &[])
                .await?;
            parse::discography(&text, band)
        })
        .await
    }

    /// Refreshes the user's complete library, optionally falling back to an explicit local seed.
    pub async fn history(
        &self,
        username: &str,
        api_key: &str,
        seed: Option<PathBuf>,
    ) -> Result<History> {
        let username_key = username.to_lowercase();
        let owner_key = "metal:lastfm:seed-owner";
        {
            let _operation = self.operation.lock().await;
            if seed.as_ref().is_some_and(|path| path.is_file())
                && self.cache.read(owner_key)?.is_none()
            {
                self.cache.write(owner_key, &username_key, now() as i64)?;
            }
        }
        let key = format!("metal:lastfm:history:{username_key}");
        let result = self
            .cached(&key, 7 * DAY, async {
                let mut history = History {
                    source: format!("lastfm:{username}"),
                    updated_at: now(),
                    ..History::default()
                };
                let mut page = 1;
                loop {
                    let data = self
                        .lastfm(
                            "library.getArtists",
                            api_key,
                            &[
                                ("user", username.into()),
                                ("limit", "500".into()),
                                ("page", page.to_string()),
                            ],
                        )
                        .await?;
                    let artists = data["artists"]["artist"]
                        .as_array()
                        .context("invalid Last.fm library")?;
                    for artist in artists {
                        let stats = stats(artist)?;
                        history
                            .artists
                            .entry(normalize(&stats.name))
                            .and_modify(|kept| kept.playcount = kept.playcount.max(stats.playcount))
                            .or_insert(stats);
                    }
                    let pages = number(&data["artists"]["@attr"]["totalPages"])
                        .context("Last.fm library has no page count")?;
                    if page >= pages.max(1) {
                        break;
                    }
                    if artists.is_empty() {
                        bail!("incomplete Last.fm library pagination");
                    }
                    page += 1;
                    if page > 1000 {
                        bail!("Last.fm library exceeds pagination safety limit");
                    }
                }
                Ok(history)
            })
            .await;
        match result {
            Ok(history) => Ok(history),
            Err(error) => match seed {
                Some(path) => {
                    let _operation = self.operation.lock().await;
                    if self
                        .cache
                        .read(owner_key)?
                        .is_some_and(|owner| owner != username_key)
                    {
                        return Err(error);
                    }
                    let history = parse::seed(
                        &std::fs::read_to_string(path)
                            .context("cannot read explicit Last.fm seed")?,
                        username,
                    )?;
                    self.cache.write(owner_key, &username_key, now() as i64)?;
                    Ok(history)
                }
                None => Err(error),
            },
        }
    }

    pub async fn artist_info(&self, name: &str, api_key: &str) -> Result<ArtistStats> {
        self.cached(
            &format!("metal:lastfm:artist:{}", normalize(name)),
            30 * DAY,
            async {
                let found = self
                    .lastfm("artist.getInfo", api_key, &[("artist", name.into())])
                    .await;
                let data = match found {
                    Err(error)
                        if error
                            .downcast_ref::<LastfmError>()
                            .is_some_and(|error| error.0 == 6) =>
                    {
                        return Ok(ArtistStats {
                            name: name.into(),
                            playcount: 0,
                            listeners: None,
                            tags: Vec::new(),
                            url: String::new(),
                        });
                    }
                    other => other?,
                };
                let artist = &data["artist"];
                let mut info = stats(artist)?;
                info.playcount = 0;
                info.listeners = number(&artist["stats"]["listeners"]);
                info.tags = artist["tags"]["tag"]
                    .as_array()
                    .map(|tags| {
                        tags.iter()
                            .filter_map(|t| t["name"].as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(info)
            },
        )
        .await
    }

    async fn lastfm(
        &self,
        method: &str,
        api_key: &str,
        params: &[(&str, String)],
    ) -> Result<Value> {
        let mut query = params.to_vec();
        query.extend([
            ("method", method.into()),
            ("api_key", api_key.into()),
            ("format", "json".into()),
        ]);
        tokio::time::sleep(Duration::from_millis(250)).await;
        let response = self
            .http
            .get("https://ws.audioscrobbler.com/2.0/")
            .query(&query)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("cannot contact Last.fm"))?;
        if !response.status().is_success() {
            bail!("Last.fm returned HTTP {}", response.status().as_u16());
        }
        let data: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("cannot decode Last.fm response"))?;
        if let Some(code) = number(&data["error"]) {
            return Err(LastfmError(code).into());
        }
        Ok(data)
    }

    /// Reserves each request durably before awaiting HTTP, including canceled requests.
    async fn ma(&self, path: &str, query: &[(&str, String)]) -> Result<String> {
        let mut policy: Policy = self
            .cache
            .read(POLICY_KEY)?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        if policy.blocked_until > now() {
            bail!(
                "Metal Archives is cooling down until {}",
                policy.blocked_until
            );
        }
        if policy.next_request > now() {
            tokio::time::sleep(Duration::from_secs(policy.next_request - now())).await;
        }
        let at = now();
        if policy.day != at / DAY {
            policy.day = at / DAY;
            policy.requests = 0;
        }
        if policy.requests >= 30 {
            bail!("Metal Archives daily request budget exhausted");
        }
        policy.requests += 1;
        // Cover the full timeout as well as spacing if the future is canceled in flight.
        policy.next_request = at + 34;
        self.cache
            .write(POLICY_KEY, &serde_json::to_string(&policy)?, at as i64)?;
        let response = self
            .http
            .get(format!("{MA}{path}"))
            .query(query)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("cannot contact Metal Archives"))?;
        let status = response.status();
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(retry_after)
            .unwrap_or(0);
        if status.as_u16() == 403 || status.as_u16() == 429 {
            policy.blocked_until = now().saturating_add(retry.max(DAY));
            self.cache
                .write(POLICY_KEY, &serde_json::to_string(&policy)?, now() as i64)?;
            bail!("Metal Archives blocked the request; cooldown persisted");
        }
        if retry > 0 {
            policy.blocked_until = now().saturating_add(retry.max(3600));
            self.cache
                .write(POLICY_KEY, &serde_json::to_string(&policy)?, now() as i64)?;
        }
        let text = response
            .text()
            .await
            .map_err(|_| anyhow::anyhow!("cannot read Metal Archives response"))?;
        let lower = text.to_lowercase();
        if lower.contains("cf-chl-")
            || lower.contains("<title>just a moment")
            || (lower.contains("<form") && lower.contains("captcha"))
            || lower.contains("challenge-platform")
        {
            policy.blocked_until = now().saturating_add(retry.max(DAY));
            self.cache
                .write(POLICY_KEY, &serde_json::to_string(&policy)?, now() as i64)?;
            bail!("Metal Archives challenge encountered; cooldown persisted");
        }
        if !status.is_success() {
            bail!("Metal Archives returned HTTP {}", status.as_u16());
        }
        policy.next_request = now() + 4;
        self.cache
            .write(POLICY_KEY, &serde_json::to_string(&policy)?, now() as i64)?;
        Ok(text)
    }
}

impl std::fmt::Display for LastfmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Last.fm API error {}", self.0)
    }
}

impl std::error::Error for LastfmError {}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn rows(value: &Value) -> Result<&Vec<Value>> {
    value["aaData"]
        .as_array()
        .context("Metal Archives response has no data rows")
}
fn stats(value: &Value) -> Result<ArtistStats> {
    Ok(ArtistStats {
        name: value["name"]
            .as_str()
            .context("Last.fm artist has no name")?
            .into(),
        playcount: number(&value["playcount"]).unwrap_or(0),
        listeners: None,
        tags: Vec::new(),
        url: value["url"].as_str().unwrap_or_default().into(),
    })
}
fn retry_after(value: &str) -> u64 {
    value.parse().unwrap_or_else(|_| {
        jiff::fmt::rfc2822::parse(value)
            .map(|date| (date.timestamp().as_second().max(0) as u64).saturating_sub(now()))
            .unwrap_or(DAY)
    })
}
