mod albums;
mod empath;
mod parse;

pub use albums::{
    AlbumLibrary, AlbumMatch, AlbumPlays, ListenedAlbum, album_search_title, album_title_score,
    match_album_title,
};
pub use empath::{
    EmpathBand, EmpathCached, EmpathClient, EmpathGenres, EmpathRank, EmpathSimilar, EmpathStyle,
};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    io::Read as _,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, OwnedMutexGuard};

const DAY: u64 = 86_400;
const REQUEST_INTERVAL: u64 = 2;
const DAILY_REQUEST_LIMIT: u32 = 100;
const MA: &str = "https://www.metal-archives.com";
const POLICY_KEY: &str = "metal:policy:v2";
const LEGACY_POLICY_KEY: &str = "metal:policy:v1";
const LASTFM_RETRY_KEY: &str = "metal:lastfm:retry-until:v1";
const LASTFM_INTERVAL: Duration = Duration::from_secs(1);

/// The refusal that caused a persisted pause; old caches retain an unknown reason.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum AccessReason {
    Forbidden,
    RateLimited,
    Challenge,
    RetryAfter,
    DailyBudget,
    Unknown,
}

/// A Metal Archives refusal with a retry time, without response bodies or sensitive headers.
#[derive(Clone, Copy, Debug)]
pub struct AccessError {
    pub reason: AccessReason,
    pub retry_at: u64,
}

/// A user-visible browser lookup that could not return complete artist metadata.
#[derive(Clone, Copy, Debug)]
pub enum BrowserError {
    Unavailable,
    Closed,
    WrongArtist,
    Incomplete,
    TimedOut,
}

/// An artist lookup that can be restored locally without contacting Metal Archives.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtistMetadata {
    pub candidates: Vec<Band>,
    pub band: Option<Band>,
    pub releases: Vec<Release>,
    #[serde(default)]
    pub browser: bool,
}

/// A locally restored response and whether its 24-hour freshness window has elapsed.
pub struct Cached<T> {
    pub data: T,
    pub expired: bool,
}

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

/// Whether the available date proves a release is out, upcoming, or still uncertain.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAvailability {
    Released,
    Upcoming,
    Unconfirmed,
}

impl Release {
    /// Compares full dates or date ranges without treating the current year as released.
    pub fn availability(&self, today: jiff::civil::Date) -> ReleaseAvailability {
        if let Ok(date) = self.date.parse::<jiff::civil::Date>() {
            return match date > today {
                true => ReleaseAvailability::Upcoming,
                false => ReleaseAvailability::Released,
            };
        }
        let bounds = self
            .date
            .split_once('-')
            .and_then(|(year, month)| {
                let first =
                    jiff::civil::Date::new(year.parse().ok()?, month.parse().ok()?, 1).ok()?;
                let last =
                    jiff::civil::Date::new(first.year(), first.month(), first.days_in_month())
                        .ok()?;
                Some((first, last))
            })
            .or_else(|| {
                let year = self.date.parse::<i16>().ok()?;
                Some((
                    jiff::civil::Date::new(year, 1, 1).ok()?,
                    jiff::civil::Date::new(year, 12, 31).ok()?,
                ))
            });
        match bounds {
            Some((first, _)) if first > today => ReleaseAvailability::Upcoming,
            Some((_, last)) if last < today => ReleaseAvailability::Released,
            _ => ReleaseAvailability::Unconfirmed,
        }
    }

    /// Recent year-only dates need an album-page lookup; historical years already prove release.
    pub fn needs_date(&self, today: jiff::civil::Date) -> bool {
        self.date.parse::<jiff::civil::Date>().is_err()
            && self
                .date
                .split('-')
                .next()
                .and_then(|year| year.parse::<i16>().ok())
                .is_some_and(|year| year >= today.year())
    }
}

/// Reserved requests and an exclusive Archives lease, held until completion or cancellation.
pub struct BrowserBudget {
    pub additional: usize,
    day: u64,
    next_request: u64,
    client: Client,
    _operation: OwnedMutexGuard<()>,
    finished: bool,
}

impl Drop for BrowserBudget {
    fn drop(&mut self) {
        // A canceled browser may have just sent a request. Keep its reserved slots and leave
        // the same cooldown as a completed browser before releasing the operation lease.
        if !self.finished
            && let Err(error) = self.client.finish_browser(self, None, None)
        {
            log::warn!("metal: cannot persist browser cooldown: {error:#}");
        }
    }
}

/// A release listing whose advertised total also identifies partial legacy caches.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseFeed {
    pub releases: Vec<Release>,
    pub total: usize,
    /// Remembers a working browser fetch so refreshes avoid repeating a refused HTTP request.
    #[serde(default)]
    pub browser: bool,
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

/// A persisted Last.fm rate-limit pause shared by all metadata requests and accounts.
#[derive(Clone, Copy, Debug)]
pub struct LastfmAccessError {
    pub retry_at: u64,
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

/// Opens the site's ordinary search page, encoding the artist name as query parameters.
pub fn archives_search_url(name: &str) -> String {
    let mut url =
        reqwest::Url::parse("https://www.metal-archives.com/search").expect("valid Archives URL");
    url.query_pairs_mut()
        .append_pair("searchString", name)
        .append_pair("type", "band_name");
    url.into()
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
    #[serde(default)]
    blocked_reason: Option<AccessReason>,
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
    lastfm_operation: Arc<Mutex<()>>,
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
            lastfm_operation: Arc::new(Mutex::new(())),
        }
    }

    fn snapshot<T: DeserializeOwned>(&self, key: &str) -> Result<Option<Snapshot<T>>> {
        self.cache
            .read(key)?
            .map(|s| serde_json::from_str(&s).context("cannot decode metal cache"))
            .transpose()
    }

    fn save<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.save_at(key, value, now())
    }

    /// Keeps a model's freshness timestamp identical to its persisted snapshot deadline.
    fn save_at<T: Serialize>(&self, key: &str, value: &T, at: u64) -> Result<()> {
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
        if let Some(cached) = self.fresh(key, ttl)? {
            return Ok(cached);
        }
        let _operation = self.lastfm_operation.lock().await;
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

    /// Returns a fresh local value without waiting for any in-flight network operation.
    fn fresh<T: DeserializeOwned>(&self, key: &str, ttl: u64) -> Result<Option<T>> {
        Ok(self
            .snapshot::<T>(key)?
            .filter(|snapshot| now().saturating_sub(snapshot.at) < ttl)
            .map(|snapshot| snapshot.value))
    }

    /// Reads metadata locally; expiration never triggers a network request by itself.
    fn preview<T: DeserializeOwned>(&self, key: &str) -> Result<Option<Cached<T>>> {
        Ok(self.snapshot(key)?.map(|snapshot| Cached {
            data: snapshot.value,
            expired: now().saturating_sub(snapshot.at) >= DAY,
        }))
    }

    /// Fetches only on explicit demand, with a 24-hour cache and a real refresh override.
    async fn archives_cached<T, F>(&self, key: &str, refresh: bool, fetch: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: Future<Output = Result<T>>,
    {
        if !refresh && let Some(cached) = self.fresh(key, DAY)? {
            return Ok(cached);
        }
        let _operation = self.operation.lock().await;
        if !refresh && let Some(cached) = self.fresh(key, DAY)? {
            return Ok(cached);
        }
        let value = fetch.await?;
        self.save(key, &value)?;
        Ok(value)
    }

    /// Restores an artist's last successful selection and results without any HTTP work.
    pub fn cached_artist(&self, name: &str) -> Result<Option<Cached<ArtistMetadata>>> {
        if let Some(cached) = self.preview(&format!("metal:artist:{}", normalize(name)))? {
            return Ok(Some(cached));
        }
        let Some(candidates) =
            self.preview::<Vec<Band>>(&format!("metal:bands:{}", normalize(name)))?
        else {
            return Ok(None);
        };
        let discography = match candidates.data.as_slice() {
            [band] => self.preview::<Vec<Release>>(&format!("metal:discography:{}", band.id))?,
            _ => None,
        };
        let expired = candidates.expired || discography.as_ref().is_some_and(|cache| cache.expired);
        let band = discography
            .as_ref()
            .and_then(|_| candidates.data.first().cloned());
        Ok(Some(Cached {
            data: ArtistMetadata {
                candidates: candidates.data,
                band,
                releases: discography.map(|cache| cache.data).unwrap_or_default(),
                browser: false,
            },
            expired,
        }))
    }

    /// Restores a specific source identity without accepting another namesake's saved selection.
    /// Per-band snapshots remain reusable even if the name cache was replaced by another artist.
    pub fn cached_artist_for(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> Result<Option<Cached<ArtistMetadata>>> {
        let cached = self.cached_artist(name)?;
        let Some(id) = id else {
            return Ok(cached);
        };
        if let Some(cached) = cached
            && cached.data.band.as_ref().is_some_and(|band| band.id == id)
        {
            return Ok(Some(cached));
        }
        let Some(band) = self.preview::<Band>(&format!("metal:band-id:{id}"))? else {
            return Ok(None);
        };
        if normalize(&band.data.name) != normalize(name) {
            return Ok(None);
        }
        let Some(releases) = self.preview::<Vec<Release>>(&format!("metal:discography:{id}"))?
        else {
            return Ok(None);
        };
        Ok(Some(Cached {
            expired: band.expired || releases.expired,
            data: ArtistMetadata {
                candidates: vec![band.data.clone()],
                band: Some(band.data),
                releases: releases.data,
                browser: true,
            },
        }))
    }

    /// Persists only successful lookups; a failed refresh leaves the old response intact.
    pub fn save_artist(&self, name: &str, data: &ArtistMetadata) -> Result<()> {
        let mut at = self
            .snapshot::<Vec<Band>>(&format!("metal:bands:{}", normalize(name)))?
            .map(|snapshot| snapshot.at)
            .unwrap_or_else(now);
        if let Some(band) = &data.band
            && let Some(snapshot) =
                self.snapshot::<Vec<Release>>(&format!("metal:discography:{}", band.id))?
        {
            at = at.min(snapshot.at);
        }
        self.cache.write(
            &format!("metal:artist:{}", normalize(name)),
            &serde_json::to_string(&Snapshot { at, value: data })?,
            at as i64,
        )
    }

    /// Restores a date window without refreshing it or making any network request.
    pub fn cached_releases(&self, from: &str, to: &str) -> Result<Option<Cached<ReleaseFeed>>> {
        let normal = self.preview(&format!("metal:releases:{from}:{to}"))?;
        match normal {
            Some(cached) => Ok(Some(cached)),
            None => self.preview(&format!("metal:imported-releases:{from}:{to}")),
        }
    }

    /// Fetches every remote page on demand, preserving the remote total in cached responses.
    pub async fn releases(&self, from: &str, to: &str, refresh: bool) -> Result<ReleaseFeed> {
        let start: jiff::civil::Date = from.parse().context("invalid release start date")?;
        let end: jiff::civil::Date = to.parse().context("invalid release end date")?;
        if start > end {
            bail!("release date range is reversed");
        }
        self.archives_cached(&format!("metal:releases:{from}:{to}"), refresh, async {
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
                browser: false,
            })
        })
        .await
    }

    /// Searches exact normalized band names and preserves all matching namesakes.
    pub async fn bands(&self, name: &str, refresh: bool) -> Result<Vec<Band>> {
        self.archives_cached(
            &format!("metal:bands:{}", normalize(name)),
            refresh,
            async {
                let mut bands = Vec::new();
                let mut offset = 0;
                loop {
                    let text = self
                        .ma(
                            "/search/ajax-band-search/",
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

    /// Returns the discography before date checks, caching it for 24 hours unless refreshed.
    /// Call `check_date` separately for uncertain recent dates so the list can appear immediately.
    pub async fn discography(&self, band: &Band, refresh: bool) -> Result<Vec<Release>> {
        if !band.id.chars().all(|c| c.is_ascii_digit()) || band.id.is_empty() {
            bail!("invalid Metal Archives band ID");
        }
        self.archives_cached(&format!("metal:discography:{}", band.id), refresh, async {
            let text = self
                .ma(&format!("/band/discography/id/{}/tab/all", band.id), &[])
                .await?;
            parse::discography(&text, band)
        })
        .await
    }

    /// Checks one uncertain date and updates matching saved results without extending their age.
    /// Call only after saving the base discography; historical or already exact dates do no work.
    pub async fn check_date(&self, name: &str, release: &Release) -> Result<Option<String>> {
        if !release.needs_date(jiff::Zoned::now().date()) {
            return Ok(None);
        }
        let _operation = self.operation.lock().await;
        let url = reqwest::Url::parse(&release.url)?;
        if url.origin() != reqwest::Url::parse(MA)?.origin() || !url.path().starts_with("/albums/")
        {
            bail!("invalid release date URL");
        }
        let html = self.ma(url.path(), &[]).await?;
        let Some(date) = parse::album_date(&html) else {
            return Ok(None);
        };
        let key = format!("metal:artist:{}", normalize(name));
        if let Some(mut saved) = self.snapshot::<ArtistMetadata>(&key)?
            && saved
                .value
                .band
                .as_ref()
                .is_some_and(|band| band.id == release.band.id)
            && let Some(row) = saved
                .value
                .releases
                .iter_mut()
                .find(|row| row.id == release.id)
        {
            row.date = date.clone();
            self.cache
                .write(&key, &serde_json::to_string(&saved)?, saved.at as i64)?;
        }
        let key = format!("metal:discography:{}", release.band.id);
        if let Some(mut saved) = self.snapshot::<Vec<Release>>(&key)?
            && let Some(row) = saved.value.iter_mut().find(|row| row.id == release.id)
        {
            row.date = date.clone();
            self.cache
                .write(&key, &serde_json::to_string(&saved)?, saved.at as i64)?;
        }
        Ok(Some(date))
    }

    /// Reserves a user-requested browser lookup using the same pacing and daily budget.
    pub async fn reserve_browser(&self) -> Result<()> {
        let _operation = self.operation.lock().await;
        self.reserve().await.map(|_| ())
    }

    /// Books a bounded batch of date checks up front so a browser cannot exceed the daily limit.
    pub async fn reserve_artist_browser(&self) -> Result<BrowserBudget> {
        self.reserve_browser_batch().await
    }

    /// Books the initial page and up to eight additional requests before opening the browser.
    async fn reserve_browser_batch(&self) -> Result<BrowserBudget> {
        let operation = self.operation.clone().lock_owned().await;
        let mut policy = self.reserve().await?;
        let dates = DAILY_REQUEST_LIMIT.saturating_sub(policy.requests).min(8) as usize;
        policy.requests += dates as u32;
        self.write_policy(&policy)?;
        Ok(BrowserBudget {
            additional: dates,
            day: policy.day,
            next_request: policy.next_request,
            client: self.clone(),
            _operation: operation,
            finished: false,
        })
    }

    /// Reserves the initial release-feed page and a bounded batch of pagination requests.
    pub async fn reserve_release_browser(&self) -> Result<BrowserBudget> {
        self.reserve_browser_batch().await
    }

    /// Validates every browser-fetched release and saves only a complete date window for 24 hours.
    pub async fn capture_releases(
        &self,
        from: &str,
        to: &str,
        payload: &str,
    ) -> Result<ReleaseFeed> {
        let _operation = self.operation.lock().await;
        self.save_browser_releases(from, to, payload)
    }

    /// Settles and saves a complete feed before releasing its exclusive browser lease.
    pub async fn complete_release_browser(
        &self,
        mut budget: BrowserBudget,
        from: &str,
        to: &str,
        payload: &str,
    ) -> Result<ReleaseFeed> {
        self.settle_release_browser(&mut budget, payload)?;
        self.save_browser_releases(from, to, payload)
    }

    /// Parses and saves browser feed data while the caller holds the Archives operation gate.
    fn save_browser_releases(&self, from: &str, to: &str, payload: &str) -> Result<ReleaseFeed> {
        let start: jiff::civil::Date = from.parse()?;
        let end: jiff::civil::Date = to.parse()?;
        let data = browser_feed(payload)?;
        if data["sonoraError"].is_string() {
            let policy = self.policy()?;
            let reason = match number(&data["sonoraStatus"]) {
                Some(403) => Some(AccessReason::Forbidden),
                Some(429) => Some(AccessReason::RateLimited),
                _ if policy.blocked_until > now() => Some(AccessReason::RetryAfter),
                _ => None,
            };
            return match reason {
                Some(reason) => Err(AccessError {
                    reason,
                    retry_at: policy.blocked_until,
                }
                .into()),
                None => Err(BrowserError::Incomplete.into()),
            };
        }
        let rows = rows(&data)?;
        let total =
            number(&data["iTotalDisplayRecords"]).context("browser releases have no total")?;
        if rows.len() as u64 != total || start > end {
            return Err(BrowserError::Incomplete.into());
        }
        let releases = rows
            .iter()
            .map(parse::upcoming)
            .collect::<Result<Vec<_>>>()?;
        for release in &releases {
            let date: jiff::civil::Date = release
                .date
                .parse()
                .context("browser release has no exact date")?;
            if date < start || date > end {
                return Err(BrowserError::Incomplete.into());
            }
        }
        let feed = ReleaseFeed {
            total: releases.len(),
            releases,
            browser: true,
        };
        self.save(&format!("metal:releases:{from}:{to}"), &feed)?;
        Ok(feed)
    }

    /// Returns unused pagination slots after a complete browser feed capture.
    pub async fn finish_release_browser(
        &self,
        mut budget: BrowserBudget,
        payload: &str,
    ) -> Result<()> {
        self.settle_release_browser(&mut budget, payload)
    }

    /// Applies the pagination receipt without relinquishing its browser lease.
    fn settle_release_browser(&self, budget: &mut BrowserBudget, payload: &str) -> Result<()> {
        let data = browser_feed(payload)?;
        let used = number(&data["sonoraRequests"])
            .unwrap_or(1)
            .saturating_sub(1);
        // Match the artist receipt's usize parsing even on targets narrower than u64.
        let used = usize::try_from(used).ok();
        let result = self.finish_browser(budget, used, data["sonoraRetryAfter"].as_str());
        budget.finished = result.is_ok();
        result
    }

    /// Releases unused date slots after capture and keeps two seconds after the last request.
    pub async fn finish_artist_browser(&self, mut budget: BrowserBudget, html: &str) -> Result<()> {
        self.settle_artist_browser(&mut budget, html)
    }

    /// Applies the date receipt without relinquishing its browser lease.
    fn settle_artist_browser(&self, budget: &mut BrowserBudget, html: &str) -> Result<()> {
        let result = self.finish_browser(
            budget,
            parse::browser_date_requests(html),
            parse::browser_date_retry(html).as_deref(),
        );
        budget.finished = result.is_ok();
        result
    }

    /// Settles a parsed receipt while its browser lease keeps policy writes exclusive.
    /// Without a receipt, only the cooldown changes and reserved requests remain spent.
    fn finish_browser(
        &self,
        budget: &BrowserBudget,
        used: Option<usize>,
        retry: Option<&str>,
    ) -> Result<()> {
        let used = used.unwrap_or(budget.additional).min(budget.additional);
        let mut policy = self.policy()?;
        if policy.day == budget.day {
            policy.requests = policy
                .requests
                .saturating_sub((budget.additional - used) as u32);
        }
        policy.next_request = match policy.next_request == budget.next_request {
            true => now() + REQUEST_INTERVAL,
            false => policy.next_request.max(now() + REQUEST_INTERVAL),
        };
        if let Some(wait) = retry.map(retry_after)
            && wait > 0
        {
            policy.blocked_until = policy.blocked_until.max(now().saturating_add(wait));
            policy.blocked_reason = Some(AccessReason::RetryAfter);
        }
        self.write_policy(&policy)
    }

    /// Parses the rendered band and complete discography after browser verification.
    pub async fn capture_browser(&self, name: &str, html: &str) -> Result<ArtistMetadata> {
        self.capture_browser_for(name, None, html).await
    }

    /// Saves a complete browser capture only when its name and any explicit source ID match.
    pub async fn capture_browser_for(
        &self,
        name: &str,
        id: Option<&str>,
        html: &str,
    ) -> Result<ArtistMetadata> {
        let _operation = self.operation.lock().await;
        self.save_browser_artist(name, id, html)
    }

    /// Settles and saves a validated artist capture before releasing its browser lease.
    pub async fn complete_artist_browser(
        &self,
        mut budget: BrowserBudget,
        name: &str,
        id: Option<&str>,
        html: &str,
    ) -> Result<ArtistMetadata> {
        self.settle_artist_browser(&mut budget, html)?;
        self.save_browser_artist(name, id, html)
    }

    /// Parses and saves artist data while the caller holds the Archives operation gate.
    fn save_browser_artist(
        &self,
        name: &str,
        id: Option<&str>,
        html: &str,
    ) -> Result<ArtistMetadata> {
        let data = Self::browser_metadata_for(name, id, html)?;
        let band = data.band.as_ref().expect("browser metadata has a band");
        self.save(&format!("metal:band-id:{}", band.id), band)?;
        self.save(&format!("metal:discography:{}", band.id), &data.releases)?;
        self.save(&format!("metal:artist:{}", normalize(name)), &data)?;
        Ok(data)
    }

    /// Validates an intermediate browser capture without saving it as a completed refresh.
    pub fn browser_metadata(name: &str, html: &str) -> Result<ArtistMetadata> {
        Self::browser_metadata_for(name, None, html)
    }

    /// Validates an intermediate capture against both the artist name and selected source ID.
    pub fn browser_metadata_for(
        name: &str,
        id: Option<&str>,
        html: &str,
    ) -> Result<ArtistMetadata> {
        let band = parse::browser_band(html)?;
        if normalize(&band.name) != normalize(name) || id.is_some_and(|id| id != band.id) {
            return Err(BrowserError::WrongArtist.into());
        }
        let releases = parse::browser_discography(html, &band)?;
        Ok(ArtistMetadata {
            candidates: vec![band.clone()],
            band: Some(band.clone()),
            releases,
            browser: true,
        })
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
            let _operation = self.lastfm_operation.lock().await;
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
                    let _operation = self.lastfm_operation.lock().await;
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

    /// Restores the last complete account-wide album chart without requesting the network.
    pub fn cached_album_library(&self, user: &str) -> Result<Option<Cached<AlbumLibrary>>> {
        self.preview(&album_library_key(user))
    }

    /// Imports the overall chart once, then refreshes only recently played albums each day.
    /// Failed batches retain the prior snapshot; unresolved albums are retried on later syncs.
    pub async fn album_library(&self, user: &str, api_key: &str) -> Result<AlbumLibrary> {
        let key = album_library_key(user);
        if let Some(cached) = self.fresh(&key, DAY)? {
            return Ok(cached);
        }
        let _operation = self.lastfm_operation.lock().await;
        if let Some(cached) = self.fresh(&key, DAY)? {
            return Ok(cached);
        }
        if let Some(snapshot) = self.snapshot::<AlbumLibrary>(&key)? {
            let mut library = snapshot.value;
            self.refresh_album_library(user, api_key, &mut library)
                .await?;
            self.save_at(&key, &library, library.fetched_at)?;
            return Ok(library);
        }
        let mut library = AlbumLibrary {
            artists: HashMap::new(),
            fetched_at: now(),
            pending: Vec::new(),
        };
        let mut seen = HashSet::new();
        let mut expected = None;
        let mut page = 1;
        loop {
            let data = self
                .lastfm(
                    "user.getTopAlbums",
                    api_key,
                    &[
                        ("user", user.into()),
                        ("period", "overall".into()),
                        ("limit", "1000".into()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let chart = albums::AlbumPage::parse(user, page, &data)?;
            let bounds = (chart.pages, chart.total);
            if expected.is_some_and(|expected| expected != bounds) {
                bail!("Last.fm album chart changed during pagination");
            }
            expected = Some(bounds);
            let empty = chart.albums.is_empty();
            for (artist, title, count) in chart.albums {
                if !seen.insert((artist.clone(), title.clone())) {
                    bail!("Last.fm album chart repeated an album during pagination");
                }
                library.insert(&artist, title, count);
            }
            if page >= chart.pages.max(1) {
                if seen.len() as u64 != chart.total {
                    bail!("incomplete Last.fm album chart");
                }
                break;
            }
            if empty {
                bail!("incomplete Last.fm album chart pagination");
            }
            page += 1;
        }
        self.save_at(&key, &library, library.fetched_at)?;
        Ok(library)
    }

    /// Uses recent listens to select albums, then replaces their totals rather than adding plays.
    async fn refresh_album_library(
        &self,
        user: &str,
        api_key: &str,
        library: &mut AlbumLibrary,
    ) -> Result<()> {
        let until = now();
        let yesterday = jiff::Zoned::now()
            .checked_sub(jiff::Span::new().days(1))?
            .start_of_day()?
            .timestamp()
            .as_second()
            .max(0) as u64;
        // A longer absence extends the range to the last successful import or refresh.
        let from = library.fetched_at.min(yesterday);
        let mut albums = HashMap::new();
        for (artist, title) in &library.pending {
            albums.insert(
                (normalize(artist), normalize(title)),
                (artist.clone(), title.clone()),
            );
        }
        let mut expected = None;
        let mut scrobbles = 0;
        let mut page = 1;
        loop {
            let data = self
                .lastfm(
                    "user.getRecentTracks",
                    api_key,
                    &[
                        ("user", user.into()),
                        ("limit", "200".into()),
                        ("page", page.to_string()),
                        ("from", from.saturating_sub(1).to_string()),
                        ("to", until.saturating_add(1).to_string()),
                    ],
                )
                .await?;
            let recent = albums::RecentAlbumPage::parse(user, page, from, until, &data)?;
            let bounds = (recent.pages, recent.total);
            if expected.is_some_and(|expected| expected != bounds) {
                bail!("Last.fm recent tracks changed during pagination");
            }
            expected = Some(bounds);
            scrobbles += recent.scrobbles;
            for (artist, title) in recent.albums {
                albums
                    .entry((normalize(&artist), normalize(&title)))
                    .or_insert((artist, title));
            }
            if page >= recent.pages.max(1) {
                if scrobbles != recent.total {
                    bail!("incomplete Last.fm recent tracks");
                }
                break;
            }
            if recent.scrobbles == 0 {
                bail!("incomplete Last.fm recent tracks pagination");
            }
            page += 1;
        }
        let mut albums = albums.into_values().collect::<Vec<_>>();
        albums.sort();
        for (artist, title) in albums {
            let data = self
                .lastfm(
                    "album.getInfo",
                    api_key,
                    &[
                        ("username", user.into()),
                        ("artist", artist.clone()),
                        ("album", title.clone()),
                        ("autocorrect", "0".into()),
                    ],
                )
                .await;
            let resolved = match data {
                Ok(data) => library.refresh_album(&artist, &title, &data)?,
                Err(error)
                    if error
                        .downcast_ref::<LastfmError>()
                        .is_some_and(|code| matches!(code.0, 6 | 7)) =>
                {
                    false
                }
                Err(error) => return Err(error),
            };
            if !resolved {
                library.defer_album(&artist, &title);
            }
        }
        library.fetched_at = until;
        Ok(())
    }

    async fn lastfm(
        &self,
        method: &str,
        api_key: &str,
        params: &[(&str, String)],
    ) -> Result<Value> {
        if let Some(deadline) = self.cache.read(LASTFM_RETRY_KEY)? {
            let retry_at: u64 = deadline
                .parse()
                .context("cannot decode Last.fm retry deadline")?;
            if retry_at > now() {
                return Err(LastfmAccessError { retry_at }.into());
            }
        }
        let mut query = params.to_vec();
        query.extend([
            ("method", method.into()),
            ("api_key", api_key.into()),
            ("format", "json".into()),
        ]);
        tokio::time::sleep(LASTFM_INTERVAL).await;
        let response = self
            .http
            .get("https://ws.audioscrobbler.com/2.0/")
            .query(&query)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("cannot contact Last.fm"))?;
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(self.pause_lastfm(retry.as_deref())?.into());
        }
        if !response.status().is_success() {
            bail!("Last.fm returned HTTP {}", response.status().as_u16());
        }
        let data: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("cannot decode Last.fm response"))?;
        if let Some(code) = number(&data["error"]) {
            if code == 29 {
                return Err(self.pause_lastfm(retry.as_deref())?.into());
            }
            return Err(LastfmError(code).into());
        }
        Ok(data)
    }

    /// Honors server retry instructions, or pauses for a minute when none are supplied.
    fn pause_lastfm(&self, retry: Option<&str>) -> Result<LastfmAccessError> {
        let wait = retry
            .map(retry_after)
            .filter(|wait| *wait > 0)
            .unwrap_or(60);
        let retry_at = now().saturating_add(wait);
        self.cache
            .write(LASTFM_RETRY_KEY, &retry_at.to_string(), now() as i64)?;
        Ok(LastfmAccessError { retry_at })
    }

    /// Migrates the old artificial refusal timer while retaining request accounting.
    fn policy(&self) -> Result<Policy> {
        if let Some(value) = self.cache.read(POLICY_KEY)? {
            return serde_json::from_str(&value).context("cannot decode request policy");
        }
        let mut policy: Policy = self
            .cache
            .read(LEGACY_POLICY_KEY)?
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        policy.blocked_until = 0;
        policy.blocked_reason = None;
        self.write_policy(&policy)?;
        Ok(policy)
    }

    fn write_policy(&self, policy: &Policy) -> Result<()> {
        self.cache
            .write(POLICY_KEY, &serde_json::to_string(policy)?, now() as i64)
    }

    /// Persists each reservation before HTTP, including requests canceled in flight.
    async fn reserve(&self) -> Result<Policy> {
        let mut policy = self.policy()?;
        if policy.blocked_until > now() {
            return Err(AccessError {
                reason: policy.blocked_reason.unwrap_or(AccessReason::RetryAfter),
                retry_at: policy.blocked_until,
            }
            .into());
        }
        let mut at = now();
        if policy.day != at / DAY {
            policy.day = at / DAY;
            policy.requests = 0;
        }
        if policy.requests >= DAILY_REQUEST_LIMIT {
            return Err(AccessError {
                reason: AccessReason::DailyBudget,
                retry_at: (at / DAY + 1) * DAY,
            }
            .into());
        }
        if policy.next_request > at {
            tokio::time::sleep(Duration::from_secs(policy.next_request - at)).await;
            at = now();
            if policy.day != at / DAY {
                policy.day = at / DAY;
                policy.requests = 0;
            }
        }
        policy.requests += 1;
        policy.next_request = at + 30 + REQUEST_INTERVAL;
        self.write_policy(&policy)?;
        Ok(policy)
    }

    /// Uses the public AJAX request format and honors only server-specified refusal waits.
    async fn ma(&self, path: &str, query: &[(&str, String)]) -> Result<String> {
        let mut policy = self.reserve().await?;
        let response = self
            .http
            .get(format!("{MA}{path}"))
            .header(reqwest::header::ACCEPT, "application/json, text/html;q=0.9")
            .header("X-Requested-With", "XMLHttpRequest")
            .header(reqwest::header::REFERER, format!("{MA}/"))
            .query(query)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("cannot contact Metal Archives"))?;
        let status = response.status();
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(retry_after)
            .unwrap_or(0);
        let text = response
            .text()
            .await
            .map_err(|_| anyhow::anyhow!("cannot read Metal Archives response"))?;
        policy.next_request = now() + REQUEST_INTERVAL;
        policy.blocked_until = if retry > 0 {
            now().saturating_add(retry)
        } else {
            0
        };
        policy.blocked_reason = (retry > 0).then_some(AccessReason::RetryAfter);
        self.write_policy(&policy)?;
        let reason = if challenge(&text) {
            Some(AccessReason::Challenge)
        } else {
            match status.as_u16() {
                403 => Some(AccessReason::Forbidden),
                429 => Some(AccessReason::RateLimited),
                _ => None,
            }
        };
        if let Some(reason) = reason {
            return Err(AccessError {
                reason,
                retry_at: policy.blocked_until,
            }
            .into());
        }
        if !status.is_success() {
            bail!("Metal Archives returned HTTP {}", status.as_u16());
        }
        Ok(text)
    }
}

impl std::fmt::Display for LastfmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Last.fm API error {}", self.0)
    }
}

impl std::error::Error for LastfmError {}

impl std::fmt::Display for LastfmAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Last.fm rate limited; retry after {}", self.retry_at)
    }
}

impl std::error::Error for LastfmAccessError {}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Metal Archives {:?}; retry after {}",
            self.reason, self.retry_at
        )
    }
}

impl std::error::Error for AccessError {}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Metal Archives browser lookup failed: {self:?}")
    }
}

impl std::error::Error for BrowserError {}

fn challenge(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("cf-chl-")
        || lower.contains("<title>just a moment")
        || (lower.contains("<form") && lower.contains("captcha"))
        || lower.contains("challenge-platform")
}

/// Decodes the complete release table carried by the native browser's bounded cookie bridge.
fn browser_feed(payload: &str) -> Result<Value> {
    let text = match payload.strip_prefix("gzip:") {
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded)?;
            let mut text = String::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .take(4_000_001)
                .read_to_string(&mut text)?;
            if text.len() > 4_000_000 {
                return Err(BrowserError::Incomplete.into());
            }
            text
        }
        None => payload.to_owned(),
    };
    serde_json::from_str(&text).context("cannot decode browser releases")
}

/// Versions the complete overall chart independently from obsolete per-album snapshots.
fn album_library_key(user: &str) -> String {
    format!(
        "metal:lastfm:albums:overall:v1:{}",
        user.trim().to_lowercase()
    )
}

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
            .unwrap_or(0)
    })
}
