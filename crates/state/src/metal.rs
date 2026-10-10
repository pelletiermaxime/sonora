use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use gpui::{Context, Entity, Task};
use jiff::civil::Date;
use metal::{
    AccessError, AccessReason, AlbumLibrary, AlbumMatch, AlbumPlays, ArtistMetadata, ArtistStats,
    Band, BrowserBudget, BrowserError, Cached, Client, History, LastfmAccessError, Release,
    ReleaseAvailability, ReleaseFeed, album_search_title, match_album_title, normalize,
};
use music::Album;
use tokio::task::AbortHandle;

use crate::{AppSettings, Io, Origin, Session, SessionEvent, Sonora, join};

const BROWSER_TICK: Duration = Duration::from_millis(250);
const BROWSER_TIMEOUT: Duration = Duration::from_secs(300);
const BROWSER_DONE: &str = "_sonora_metal_done";
const BROWSER_LANDING: &str = "www.metal-archives.com/_sonora_metal_capture";
const ALBUM_RETRY_DELAY: u64 = 30;

/// Keeps stale results available while a release-window refresh uses HTTP or browser verification.
enum DiscoverLoad {
    Ready(Cached<ReleaseFeed>),
    Browser(Option<Cached<ReleaseFeed>>),
    Failed(Option<Cached<ReleaseFeed>>, anyhow::Error),
}

/// Distinguishes provisional cached identity from a selected HTTP or browser result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArtistLoad {
    Cache,
    Fetch,
    BrowserCapture,
}

/// Retains structured HTTP and browser failures for localization when a view renders.
#[derive(Clone)]
pub struct MetalFetchError {
    pub access: Option<AccessError>,
    pub browser: Option<BrowserError>,
}

impl From<anyhow::Error> for MetalFetchError {
    fn from(error: anyhow::Error) -> Self {
        log::warn!("metal: {error:#}");
        Self {
            access: error.downcast_ref::<AccessError>().copied(),
            browser: error.downcast_ref::<BrowserError>().copied(),
        }
    }
}

/// Shared release discovery, listening history, and on-demand streaming album resolution.
pub struct MetalDiscovery {
    pub releases: Vec<Release>,
    pub history: History,
    pub info: HashMap<String, ArtistStats>,
    pub loading: bool,
    pub enriching: bool,
    pub error: Option<MetalFetchError>,
    pub history_error: Option<String>,
    pub enrichment_error: Option<String>,
    pub from: Date,
    pub to: Date,
    pub matching: Option<Release>,
    pub albums: Vec<Album>,
    pub resolving: bool,
    pub album_error: Option<&'static str>,
    pub release_total: usize,
    pub expired: bool,
    pub reader_error: Option<MetalFetchError>,
    client: Client,
    session: Entity<Session>,
    settings: Entity<AppSettings>,
    io: Io,
    loaded: bool,
    follows_today: bool,
    user: String,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
    history_task: Option<Task<()>>,
    history_request: Option<AbortHandle>,
    history_loading: bool,
    enrichment: Option<Task<()>>,
    enrichment_request: Option<AbortHandle>,
    resolve: Option<Task<()>>,
    resolve_request: Option<AbortHandle>,
    resolved: HashMap<String, Vec<Album>>,
    reader: Option<webview::Page>,
    reader_task: Option<Task<()>>,
    reader_request: Option<AbortHandle>,
    feed_page: Option<webview::Page>,
    feed_poll: Option<Task<()>>,
    feed_opened: Option<Instant>,
    feed_budget: Option<BrowserBudget>,
    pub album_plays_loading: bool,
    pub album_plays_failed: bool,
    pub album_plays_retry_at: Option<u64>,
    album_library: Option<AlbumLibrary>,
    album_account: Option<(String, String)>,
    album_task: Option<Task<()>>,
    album_request: Option<AbortHandle>,
    album_retry_until: u64,
}

impl MetalDiscovery {
    /// Creates a shared client without starting any network work until Discover opens.
    pub fn new(
        session: Entity<Session>,
        settings: Entity<AppSettings>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&session, |this, _, event, cx| {
            if matches!(event, SessionEvent::SignedOut) {
                this.cancel();
                this.resolved.clear();
                this.history = History::default();
                this.info.clear();
                this.user.clear();
                this.loaded = false;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&settings, |this, settings, cx| {
            let account = lastfm_album_account(settings.read(cx));
            let albums_used = this.album_library.is_some() || this.album_plays_loading;
            let user = settings
                .read(cx)
                .scrobbling()
                .get("lastfm")
                .map(|account| account.name.clone())
                .unwrap_or_default();
            if user != this.user {
                let used = this.loaded || this.loading;
                this.cancel();
                this.history = History::default();
                this.info.clear();
                this.loaded = false;
                this.user = user;
                if used {
                    this.load(cx);
                }
                cx.notify();
            }
            if account != this.album_account {
                this.clear_album_history();
                this.album_account = account;
                if albums_used {
                    this.load_album_history(cx);
                }
                cx.notify();
            }
        })
        .detach();
        let to = jiff::Zoned::now().date();
        let album_account = lastfm_album_account(settings.read(cx));
        let from = to
            .checked_sub(jiff::Span::new().days(6))
            .expect("recent date is representable");
        Self {
            releases: Vec::new(),
            history: History::default(),
            info: HashMap::new(),
            loading: false,
            enriching: false,
            error: None,
            history_error: None,
            enrichment_error: None,
            from,
            to,
            matching: None,
            albums: Vec::new(),
            resolving: false,
            album_error: None,
            release_total: 0,
            expired: false,
            reader_error: None,
            client: Client::new(storage::Cache::standard()),
            session,
            settings,
            io,
            loaded: false,
            follows_today: true,
            user: String::new(),
            task: None,
            request: None,
            history_task: None,
            history_request: None,
            history_loading: false,
            enrichment: None,
            enrichment_request: None,
            resolve: None,
            resolve_request: None,
            resolved: HashMap::new(),
            reader: None,
            reader_task: None,
            reader_request: None,
            feed_page: None,
            feed_poll: None,
            feed_opened: None,
            feed_budget: None,
            album_plays_loading: false,
            album_plays_failed: false,
            album_plays_retry_at: None,
            album_library: None,
            album_account,
            album_task: None,
            album_request: None,
            album_retry_until: 0,
        }
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Returns a locally matched count from the last complete personal album chart.
    pub fn album_plays(&self, release: &Release) -> Option<AlbumPlays> {
        if release.availability(jiff::Zoned::now().date()) != ReleaseAvailability::Released {
            return None;
        }
        self.album_library
            .as_ref()
            .map(|library| library.plays(&release.band.name, &release.title))
    }

    /// Loads one shared account chart; changing artist pages does not restart pagination.
    pub fn load_album_history(&mut self, cx: &mut Context<Self>) {
        if self.album_plays_loading
            || self
                .album_library
                .as_ref()
                .is_some_and(|library| !library.expired())
        {
            return;
        }
        let Some((user, api_key)) = self.album_account.clone() else {
            return;
        };
        let paused = self.album_retry_until > timestamp();
        self.album_plays_loading = true;
        if !paused {
            self.album_plays_failed = false;
            self.album_plays_retry_at = None;
        }
        let client = self.client.clone();
        let (send, mut updates) = tokio::sync::mpsc::channel(1);
        let request = self.io.spawn(async move {
            let cached = client.cached_album_library(&user)?;
            let fresh = cached.as_ref().is_some_and(|cached| !cached.expired);
            if let Some(cached) = cached
                && send.send(cached.data).await.is_err()
            {
                return Ok(());
            }
            if !fresh && !paused {
                let library = client.album_library(&user, &api_key).await?;
                let _ = send.send(library).await;
            }
            Ok(())
        });
        self.album_request = Some(request.abort_handle());
        self.album_task = Some(cx.spawn(async move |this, cx| {
            while let Some(library) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.album_library = Some(library);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.album_request = None;
                this.album_task = None;
                this.album_plays_loading = false;
                if let Err(error) = result {
                    log::warn!("metal: cannot load Last.fm album library: {error:#}");
                    this.album_plays_failed = true;
                    if !paused {
                        this.album_plays_retry_at = error
                            .downcast_ref::<LastfmAccessError>()
                            .map(|error| error.retry_at);
                        this.album_retry_until = this
                            .album_plays_retry_at
                            .unwrap_or(timestamp() + ALBUM_RETRY_DELAY);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Cancels account-owned work and removes personal memory on sign-out or account changes.
    fn clear_album_history(&mut self) {
        if let Some(request) = self.album_request.take() {
            request.abort();
        }
        self.album_task = None;
        self.album_library = None;
        self.album_plays_loading = false;
        self.album_plays_failed = false;
        self.album_plays_retry_at = None;
        self.album_retry_until = 0;
    }

    /// Automatically loads the date window, reusing fresh results for 24 hours.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let today = jiff::Zoned::now().date();
        if self.follows_today && self.to != today {
            self.cancel_load();
            self.to = today;
            self.from = today
                .checked_sub(jiff::Span::new().days(6))
                .expect("recent date is representable");
            self.releases.clear();
            self.expired = false;
            self.release_total = 0;
            self.loaded = false;
        }
        if self.loading {
            return;
        }
        self.load_mode(false, cx);
    }

    pub fn has_results(&self) -> bool {
        self.loaded
    }

    /// Explicit refresh bypasses the 24-hour result cache.
    pub fn fetch(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.load_mode(refresh, cx);
    }

    fn load_mode(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        self.history_error = None;
        let from = self.from.to_string();
        let to = self.to.to_string();
        let client = self.client.clone();
        self.load_history(cx);
        let request = self.io.spawn(async move {
            let cached = client.cached_releases(&from, &to)?;
            if !refresh && cached.as_ref().is_some_and(|cached| !cached.expired) {
                return Ok(DiscoverLoad::Ready(
                    cached.expect("fresh cache was checked"),
                ));
            }
            if cached.as_ref().is_some_and(|cached| cached.data.browser) {
                return Ok(DiscoverLoad::Browser(cached));
            }
            Ok(match client.releases(&from, &to, refresh).await {
                Ok(data) => DiscoverLoad::Ready(Cached {
                    data,
                    expired: false,
                }),
                Err(error) if browser_required(&error) => DiscoverLoad::Browser(cached),
                Err(error) => DiscoverLoad::Failed(cached, error),
            })
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.request = None;
                this.task = None;
                match loaded {
                    Ok(releases) => {
                        match releases {
                            DiscoverLoad::Ready(cached) => this.apply_feed(cached),
                            DiscoverLoad::Browser(cached) => {
                                if let Some(cached) = cached {
                                    this.apply_feed(cached);
                                }
                                this.browser(cx);
                            }
                            DiscoverLoad::Failed(cached, error) => {
                                if let Some(cached) = cached {
                                    this.apply_feed(cached);
                                }
                                this.error = Some(error.into());
                            }
                        }
                        if this.loaded {
                            this.enrich(cx);
                        }
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Loads personalization independently so local releases never wait for Last.fm pagination.
    pub fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history_loading {
            return;
        }
        self.history_loading = true;
        self.history_error = None;
        let account = self.settings.read(cx).scrobbling().get("lastfm").cloned();
        self.user = account
            .as_ref()
            .map(|account| account.name.clone())
            .unwrap_or_default();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let Some(account) =
                account.filter(|account| !account.name.is_empty() && !account.key.is_empty())
            else {
                return Ok(History::default());
            };
            let seed = dirs::home_dir().map(|home| home.join(".hermes/cache/lastfm_artists.json"));
            client.history(&account.name, &account.key, seed).await
        });
        self.history_request = Some(request.abort_handle());
        self.history_task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.history_loading = false;
                this.history_request = None;
                this.history_task = None;
                match result {
                    Ok(history) => this.history = history,
                    Err(error) => this.history_error = Some(format!("{error:#}")),
                }
                if this.loaded && !this.loading {
                    this.enrich(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn apply_feed(&mut self, cached: Cached<ReleaseFeed>) {
        self.expired = cached.expired;
        self.release_total = cached.data.total;
        self.releases = cached.data.releases;
        self.loaded = true;
    }

    /// Whether a temporary release browser is waiting for verification or pagination.
    pub fn verifying(&self) -> bool {
        self.feed_page.is_some()
    }

    /// The connected account name, without internal cache-source prefixes.
    pub fn lastfm_user(&self) -> Option<&str> {
        (!self.user.is_empty()).then_some(self.user.as_str())
    }

    fn browser(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            if !webview::supported() {
                return Err(BrowserError::Unavailable.into());
            }
            client.reserve_release_browser().await
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.request = None;
                this.task = None;
                match loaded {
                    Ok(budget) => {
                        this.feed_budget = Some(budget);
                        this.open_feed(cx);
                    }
                    Err(error) => {
                        this.loading = false;
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_feed(&mut self, cx: &mut Context<Self>) {
        let mut url =
            reqwest::Url::parse("https://www.metal-archives.com/release/ajax-upcoming/json")
                .expect("valid Archives release URL");
        url.query_pairs_mut()
            .append_pair("fromDate", &self.from.to_string())
            .append_pair("toDate", &self.to.to_string())
            .append_pair("iDisplayLength", "500")
            .append_pair("iDisplayStart", "0");
        let config = serde_json::json!({
            "pages": self.feed_budget.as_ref().map_or(1, |budget| budget.additional + 1),
            "title": i18n::lookup("metal-discover-loading", None).to_string(),
            "message": i18n::lookup("metal-discover-browser-progress", None).to_string(),
        });
        let target = webview::Target {
            url: url.into(),
            landing: BROWSER_LANDING.into(),
            domain: "metal-archives.com".into(),
            proof: vec![BROWSER_DONE.into()],
            title: i18n::lookup("metal-discover-browser-title", None).to_string(),
            agent: None,
            interactive: true,
            hidden: true,
            script: Some(format!(
                "window.__sonora_metal_feed = {config};\n{}",
                include_str!("metal-releases-browser.js")
            )),
        };
        match webview::Page::open(target) {
            Ok(page) => {
                self.feed_page = Some(page);
                self.feed_opened = Some(Instant::now());
            }
            Err(error) => {
                log::warn!("metal: cannot open release browser: {error:#}");
                self.feed_budget = None;
                self.loading = false;
                self.error = Some(anyhow::Error::from(BrowserError::Unavailable).into());
                return;
            }
        }
        self.feed_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BROWSER_TICK).await;
                if !matches!(this.update(cx, |this, cx| this.poll_feed(cx)), Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn poll_feed(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.feed_page.as_mut() else {
            return false;
        };
        let error = match page.poll() {
            webview::Poll::Pending => {
                let header = page.cookies().to_owned();
                reveal_browser(page, &header);
                if self
                    .feed_opened
                    .is_some_and(|at| at.elapsed() < BROWSER_TIMEOUT)
                {
                    return true;
                }
                BrowserError::TimedOut
            }
            webview::Poll::Closed => BrowserError::Closed,
            webview::Poll::Cookies(header) => {
                self.feed_page = None;
                self.feed_opened = None;
                let budget = self.feed_budget.take();
                let client = self.client.clone();
                let from = self.from.to_string();
                let to = self.to.to_string();
                let request = self.io.spawn(async move {
                    let payload = browser_result(&header)?;
                    match budget {
                        Some(budget) => {
                            client
                                .complete_release_browser(budget, &from, &to, &payload)
                                .await
                        }
                        None => client.capture_releases(&from, &to, &payload).await,
                    }
                });
                self.request = Some(request.abort_handle());
                self.task = Some(cx.spawn(async move |this, cx| {
                    let loaded = join(request).await;
                    this.update(cx, |this, cx| {
                        this.loading = false;
                        this.request = None;
                        this.task = None;
                        match loaded {
                            Ok(data) => {
                                this.apply_feed(Cached {
                                    data,
                                    expired: false,
                                });
                                this.enrich(cx);
                            }
                            Err(error) => this.error = Some(error.into()),
                        }
                        cx.notify();
                    })
                    .ok();
                }));
                return false;
            }
        };
        self.feed_page = None;
        self.feed_opened = None;
        self.feed_budget = None;
        self.loading = false;
        self.error = Some(anyhow::Error::from(error).into());
        cx.notify();
        false
    }

    /// Changes the date window without bypassing cached responses or the shared request gate.
    pub fn shift(&mut self, days: i64, cx: &mut Context<Self>) {
        self.release_total = 0;
        let Ok(from) = self.from.checked_add(jiff::Span::new().days(days)) else {
            return;
        };
        let Ok(to) = self.to.checked_add(jiff::Span::new().days(days)) else {
            return;
        };
        self.follows_today = false;
        self.cancel_load();
        self.from = from;
        self.to = to;
        self.releases.clear();
        self.expired = false;
        self.loaded = false;
        self.load(cx);
    }

    /// Returns to the seven-day window ending today in the user's local timezone.
    pub fn today(&mut self, cx: &mut Context<Self>) {
        self.follows_today = true;
        self.release_total = 0;
        self.cancel_load();
        self.to = jiff::Zoned::now().date();
        self.from = self
            .to
            .checked_sub(jiff::Span::new().days(6))
            .expect("recent date is representable");
        self.loaded = false;
        self.expired = false;
        self.releases.clear();
        self.load(cx);
    }

    pub fn playcount(&self, band: &str) -> u64 {
        self.history
            .artists
            .get(&normalize(band))
            .map(|artist| artist.playcount)
            .unwrap_or_default()
    }

    /// Counts checked and eligible bands in the current release window, deduplicated by name.
    pub fn recommendation_progress(&self) -> (usize, usize) {
        let bands: HashSet<_> = self
            .releases
            .iter()
            .filter(|release| {
                release.kind == "Full-length" && self.playcount(&release.band.name) <= 5
            })
            .map(|release| normalize(&release.band.name))
            .collect();
        let checked = bands
            .iter()
            .filter(|name| self.info.contains_key(*name))
            .count();
        (checked, bands.len())
    }

    /// Checks every eligible band sequentially, publishing results as they arrive.
    pub fn enrich(&mut self, cx: &mut Context<Self>) {
        if self.enriching || self.history_loading {
            return;
        }
        let Some(account) = self
            .settings
            .read(cx)
            .scrobbling()
            .get("lastfm")
            .cloned()
            .filter(|account| !account.key.is_empty())
        else {
            return;
        };
        let mut bands = Vec::new();
        let mut seen = HashSet::new();
        for release in &self.releases {
            let key = normalize(&release.band.name);
            if release.kind == "Full-length"
                && self.playcount(&release.band.name) <= 5
                && !self.info.contains_key(&key)
                && seen.insert(key)
            {
                bands.push(release.band.name.clone());
            }
        }
        if bands.is_empty() {
            return;
        }
        self.enriching = true;
        self.enrichment_error = None;
        let client = self.client.clone();
        let (send, mut updates) = tokio::sync::mpsc::channel(8);
        let request = self.io.spawn(async move {
            for band in bands {
                let artist = client.artist_info(&band, &account.key).await?;
                if send.send((normalize(&band), artist)).await.is_err() {
                    break;
                }
            }
            Ok(())
        });
        self.enrichment_request = Some(request.abort_handle());
        self.enrichment = Some(cx.spawn(async move |this, cx| {
            while let Some((name, info)) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.info.insert(name, info);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.enriching = false;
                this.enrichment_request = None;
                this.enrichment = None;
                this.enrichment_error = loaded.err().map(|error| format!("{error:#}"));
                // A feed refresh can add candidates while this batch is running.
                if this.enrichment_error.is_none() && !this.loading {
                    this.enrich(cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Searches only when a listener requests a release. Results remain account-scoped in memory.
    pub fn resolve(&mut self, release: Release, cx: &mut Context<Self>) {
        if release.availability(jiff::Zoned::now().date()) != ReleaseAvailability::Released {
            return;
        }
        self.dismiss_album(cx);
        self.matching = Some(release.clone());
        let key = release_key(&release);
        if let Some(albums) = self.resolved.get(&key) {
            self.albums = albums.clone();
            cx.notify();
            return;
        }
        let Some(provider) = self.session.read(cx).client() else {
            self.album_error = Some("metal-sign-in");
            cx.notify();
            return;
        };
        self.resolving = true;
        let request = self.io.spawn(async move {
            let mut albums = provider
                .search_albums(&format!("{} {}", release.band.name, release.title))
                .await?;
            if !albums
                .iter()
                .any(|album| album_match_rank(album, &release).is_some())
                && album_search_title(&release.title) != release.title
            {
                albums.extend(
                    provider
                        .search_albums(&format!(
                            "{} {}",
                            release.band.name,
                            album_search_title(&release.title)
                        ))
                        .await?,
                );
            }
            albums.sort_by_key(|album| album_match_rank(album, &release).unwrap_or(1001));
            let mut ids = std::collections::HashSet::new();
            albums.retain(|album| ids.insert(album.id.clone()));
            albums.truncate(12);
            Ok(albums)
        });
        self.resolve_request = Some(request.abort_handle());
        self.resolve = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.resolving = false;
                this.resolve_request = None;
                this.resolve = None;
                match loaded {
                    Ok(albums) => {
                        if !albums.is_empty() {
                            this.resolved.insert(key, albums.clone());
                        }
                        this.albums = albums;
                    }
                    Err(error) => {
                        log::warn!("metal: cannot resolve streaming album: {error:#}");
                        this.album_error = Some("metal-album-lookup-failed");
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn play(&mut self, album: Album, cx: &mut Context<Self>) {
        Sonora::global(cx)
            .playback
            .clone()
            .update(cx, |playback, cx| {
                playback.play_origin(Origin::album(&album.id).named(album.name), cx);
            });
        self.dismiss_album(cx);
    }

    pub fn dismiss_album(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.resolve_request.take() {
            request.abort();
        }
        self.resolve = None;
        self.resolving = false;
        self.matching = None;
        self.albums.clear();
        self.album_error = None;
        cx.notify();
    }

    /// Returns artwork from an already resolved, matching provider album without a lookup.
    pub fn cover(&self, release: &Release) -> Option<String> {
        release_cover(release, self.resolved.get(&release_key(release))?)
    }

    /// Opens an Archives page for reading, keeping it alive until closed or signed out.
    pub fn open_archives(&mut self, url: String, cx: &mut Context<Self>) {
        if !url.starts_with("https://www.metal-archives.com/") {
            return;
        }
        self.close_reader();
        self.reader_error = None;
        let request = self.io.spawn(async { Ok(webview::supported()) });
        self.reader_request = Some(request.abort_handle());
        self.reader_task = Some(cx.spawn(async move |this, cx| {
            let supported = join(request).await;
            this.update(cx, |this, cx| {
                this.reader_request = None;
                this.reader_task = None;
                let opened = match supported {
                    Ok(true) => webview::Page::open(webview::Target {
                        url,
                        landing: "www.metal-archives.com".into(),
                        domain: "metal-archives.com".into(),
                        proof: Vec::new(),
                        title: i18n::lookup("metal-reader-title", None).to_string(),
                        agent: None,
                        script: None,
                        interactive: true,
                        hidden: false,
                    }),
                    _ => Err(BrowserError::Unavailable.into()),
                };
                match opened {
                    Ok(page) => {
                        this.reader = Some(page);
                        this.reader_task = Some(cx.spawn(async move |this, cx| {
                            loop {
                                cx.background_executor().timer(BROWSER_TICK).await;
                                let pending = this
                                    .update(cx, |this, _| {
                                        let pending = this.reader.as_mut().is_some_and(|page| {
                                            matches!(page.poll(), webview::Poll::Pending)
                                        });
                                        if !pending {
                                            this.reader = None;
                                        }
                                        pending
                                    })
                                    .unwrap_or(false);
                                if !pending {
                                    break;
                                }
                            }
                        }));
                    }
                    Err(error) => {
                        log::warn!("metal: cannot open reading browser: {error:#}");
                        this.reader_error =
                            Some(anyhow::Error::from(BrowserError::Unavailable).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn close_reader(&mut self) {
        if let Some(request) = self.reader_request.take() {
            request.abort();
        }
        self.reader_task = None;
        self.reader = None;
        self.reader_error = None;
    }

    fn cancel_load(&mut self) {
        self.feed_page = None;
        self.feed_poll = None;
        self.feed_opened = None;
        self.feed_budget = None;
        if let Some(request) = self.request.take() {
            request.abort();
        }
        if let Some(request) = self.history_request.take() {
            request.abort();
        }
        self.history_task = None;
        self.history_loading = false;
        if let Some(request) = self.enrichment_request.take() {
            request.abort();
        }
        self.task = None;
        self.enrichment = None;
        self.loading = false;
        self.enriching = false;
    }

    fn cancel(&mut self) {
        self.clear_album_history();
        self.close_reader();
        self.cancel_load();
        if let Some(request) = self.resolve_request.take() {
            request.abort();
        }
        self.resolve = None;
        self.resolving = false;
        self.matching = None;
        self.albums.clear();
    }
}

/// A cached artist discography with background date checks and an initially hidden browser fallback.
pub struct MetalArtist {
    pub name: String,
    pub candidates: Vec<Band>,
    pub band: Option<Band>,
    pub releases: Vec<Release>,
    pub loading: bool,
    pub loaded: bool,
    pub expired: bool,
    pub error: Option<MetalFetchError>,
    pub dates_loading: bool,
    pub dates_error: Option<MetalFetchError>,
    client: Client,
    io: Io,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
    page: Option<webview::Page>,
    browser_poll: Option<Task<()>>,
    browser_opened: Option<Instant>,
    browser_budget: Option<BrowserBudget>,
    via_browser: bool,
    dates_task: Option<Task<()>>,
    dates_request: Option<AbortHandle>,
    browser_preview: Option<String>,
    requested_id: Option<String>,
}

impl MetalArtist {
    /// Shares the paced client and cancels loads and browser windows on provider sign-out.
    pub fn new(client: Client, io: Io, cx: &mut Context<Self>) -> Self {
        let session = Sonora::global(cx).session.clone();
        cx.subscribe(&session, |this, _, event, cx| {
            if matches!(event, SessionEvent::SignedOut) {
                this.clear();
                this.name.clear();
                cx.notify();
            }
        })
        .detach();
        Self {
            name: String::new(),
            candidates: Vec::new(),
            band: None,
            releases: Vec::new(),
            loading: false,
            loaded: false,
            expired: false,
            error: None,
            client,
            io,
            task: None,
            request: None,
            page: None,
            browser_poll: None,
            browser_opened: None,
            browser_budget: None,
            via_browser: false,
            dates_loading: false,
            dates_error: None,
            dates_task: None,
            dates_request: None,
            browser_preview: None,
            requested_id: None,
        }
    }

    /// Restores the local snapshot; navigation never starts a Metal Archives request.
    pub fn open(&mut self, name: &str, id: Option<String>, cx: &mut Context<Self>) {
        if self.name == name && self.requested_id == id {
            return;
        }
        self.clear();
        self.name = name.to_owned();
        self.requested_id = id.clone();
        if name.is_empty() {
            cx.notify();
            return;
        }
        self.loading = true;
        let name = name.to_owned();
        let client = self.client.clone();
        let request = self
            .io
            .spawn(async move { client.cached_artist_for(&name, id.as_deref()) });
        self.receive(request, ArtistLoad::Cache, cx);
    }

    /// Explicitly fetches metadata. A failed refresh keeps the last successful discography.
    pub fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.loading || self.dates_loading || self.name.is_empty() {
            return;
        }
        if refresh && self.via_browser {
            self.browser(cx);
            return;
        }
        self.loading = true;
        self.error = None;
        let name = self.name.clone();
        let expected = self.requested_id.clone();
        let selected = self.band.clone();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            if !refresh
                && let Some(cached) = client.cached_artist_for(&name, expected.as_deref())?
                && !cached.expired
            {
                return Ok(Some(cached));
            }
            // A verified source ID needs no name search or guessed namesake selection.
            if expected.is_some() {
                return Ok(None);
            }
            let candidates = client.bands(&name, refresh).await?;
            let band = selected
                .filter(|selected| candidates.iter().any(|band| band.id == selected.id))
                .or_else(|| match candidates.as_slice() {
                    [band] => Some(band.clone()),
                    _ => None,
                });
            let releases = match &band {
                Some(band) => client.discography(band, refresh).await?,
                None => Vec::new(),
            };
            let data = ArtistMetadata {
                candidates,
                band,
                releases,
                browser: false,
            };
            client.save_artist(&name, &data)?;
            Ok(Some(Cached {
                data,
                expired: false,
            }))
        });
        self.receive(request, ArtistLoad::Fetch, cx);
    }

    /// Fetches the discography only after the listener selects a specific namesake.
    pub fn choose(&mut self, band: Band, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.cancel_dates();
        self.requested_id = Some(band.id.clone());
        self.loading = true;
        self.error = None;
        self.band = Some(band.clone());
        let candidates = self.candidates.clone();
        let browser = self.via_browser;
        let name = self.name.clone();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let releases = client.discography(&band, false).await?;
            let data = ArtistMetadata {
                candidates,
                band: Some(band),
                releases,
                browser,
            };
            client.save_artist(&name, &data)?;
            Ok(Some(Cached {
                data,
                expired: false,
            }))
        });
        self.receive(request, ArtistLoad::Fetch, cx);
    }

    /// Shows known candidates without performing another search.
    pub fn change_band(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.cancel_dates();
        self.page = None;
        self.browser_poll = None;
        self.browser_opened = None;
        self.browser_budget = None;
        self.browser_preview = None;
        self.requested_id = None;
        self.band = None;
        self.releases.clear();
        self.error = None;
        if self.via_browser {
            self.browser(cx);
            return;
        }
        cx.notify();
    }

    /// Opens the actual site for verification, then captures its rendered complete discography.
    pub fn browser(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.dates_loading || self.name.is_empty() {
            return;
        }
        self.browser_preview = None;
        self.dates_error = None;
        self.loading = true;
        self.error = None;
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            if !webview::supported() {
                return Err(BrowserError::Unavailable.into());
            }
            client.reserve_artist_browser().await
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.request = None;
                this.task = None;
                match result {
                    Ok(budget) => {
                        this.browser_budget = Some(budget);
                        this.open_browser(cx);
                    }
                    Err(error) => {
                        this.loading = false;
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn open_browser(&mut self, cx: &mut Context<Self>) {
        let url = self
            .band
            .as_ref()
            .map(|band| band.url.clone())
            .unwrap_or_else(|| match self.requested_id.as_ref() {
                Some(id) => format!("https://www.metal-archives.com/band/view/id/{id}"),
                None => metal::archives_search_url(&self.name),
            });
        let target = webview::Target {
            url,
            landing: BROWSER_LANDING.into(),
            domain: "metal-archives.com".into(),
            proof: vec![BROWSER_DONE.into()],
            title: i18n::lookup("metal-browser-title", None).to_string(),
            agent: None,
            script: Some(format!(
                "window.__sonora_metal_name = {};\nwindow.__sonora_metal_date_limit = {};\n{}",
                serde_json::to_string(&self.name).expect("artist name serializes"),
                self.browser_budget
                    .as_ref()
                    .map_or(0, |budget| budget.additional),
                include_str!("metal-browser.js"),
            )),
            interactive: true,
            hidden: true,
        };
        match webview::Page::open(target) {
            Ok(page) => {
                self.page = Some(page);
                self.browser_opened = Some(Instant::now());
            }
            Err(error) => {
                log::warn!("metal: cannot open browser: {error:#}");
                self.browser_budget = None;
                self.loading = false;
                self.error = Some(anyhow::Error::from(BrowserError::Unavailable).into());
                return;
            }
        }
        self.browser_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BROWSER_TICK).await;
                let keep_polling = this.update(cx, |this, cx| this.poll_browser(cx));
                if !matches!(keep_polling, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Only app-owned result cookies are decoded; site authentication cookies are discarded.
    fn poll_browser(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(page) = self.page.as_mut() else {
            return false;
        };
        let error = match page.poll() {
            webview::Poll::Pending => {
                let header = page.cookies().to_owned();
                reveal_browser(page, &header);
                self.apply_preview(&header, cx);
                if self
                    .browser_opened
                    .is_none_or(|at| at.elapsed() < BROWSER_TIMEOUT)
                {
                    return true;
                }
                BrowserError::TimedOut
            }
            webview::Poll::Closed => BrowserError::Closed,
            webview::Poll::Cookies(header) => {
                self.page = None;
                self.browser_opened = None;
                let client = self.client.clone();
                let name = self.name.clone();
                let expected = self.requested_id.clone();
                let budget = self.browser_budget.take();
                let request = self.io.spawn(async move {
                    let html = browser_result(&header)?;
                    let data = match budget {
                        Some(budget) => {
                            client
                                .complete_artist_browser(budget, &name, expected.as_deref(), &html)
                                .await?
                        }
                        None => {
                            client
                                .capture_browser_for(&name, expected.as_deref(), &html)
                                .await?
                        }
                    };
                    Ok(Some(Cached {
                        data,
                        expired: false,
                    }))
                });
                self.receive(request, ArtistLoad::BrowserCapture, cx);
                return false;
            }
        };
        self.page = None;
        self.browser_opened = None;
        self.browser_budget = None;
        self.loading = false;
        self.dates_loading = false;
        self.error = Some(anyhow::Error::from(error).into());
        cx.notify();
        false
    }

    fn receive(
        &mut self,
        request: tokio::task::JoinHandle<anyhow::Result<Option<Cached<ArtistMetadata>>>>,
        mode: ArtistLoad,
        cx: &mut Context<Self>,
    ) {
        let allow_browser = mode == ArtistLoad::Fetch;
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.dates_loading = false;
                this.request = None;
                this.task = None;
                match loaded {
                    Ok(Some(mut cached)) => {
                        if let Err(error) = require_band(&cached.data, this.requested_id.as_deref())
                        {
                            this.error = Some(error.into());
                            cx.notify();
                            return;
                        }
                        cached.data.releases.sort_by(|a, b| b.date.cmp(&a.date));
                        this.candidates = cached.data.candidates;
                        this.band = cached.data.band;
                        this.releases = cached.data.releases;
                        this.loaded = true;
                        this.expired = cached.expired;
                        this.via_browser = cached.data.browser;
                        if mode != ArtistLoad::Cache {
                            this.requested_id = this.band.as_ref().map(|band| band.id.clone());
                        }
                        if allow_browser && !this.via_browser {
                            this.check_dates(cx);
                        }
                    }
                    Ok(None) => {
                        if allow_browser && this.requested_id.is_some() {
                            this.browser(cx);
                        }
                    }
                    Err(error) => {
                        let fallback = allow_browser && browser_required(&error);
                        match fallback {
                            true => this.browser(cx),
                            false => this.error = Some(error.into()),
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn clear(&mut self) {
        self.cancel_dates();
        if let Some(request) = self.request.take() {
            request.abort();
        }
        self.task = None;
        self.page = None;
        self.browser_poll = None;
        self.browser_opened = None;
        self.browser_budget = None;
        self.candidates.clear();
        self.band = None;
        self.releases.clear();
        self.loading = false;
        self.loaded = false;
        self.expired = false;
        self.error = None;
        self.via_browser = false;
        self.browser_preview = None;
        self.requested_id = None;
    }

    /// Applies a validated preview while the hidden browser continues its paced date requests.
    fn apply_preview(&mut self, header: &str, cx: &mut Context<Self>) {
        let Some(marker) = cookie_value(header, "_sonora_metal_preview") else {
            return;
        };
        if self.browser_preview.as_deref() == Some(marker) {
            return;
        }
        let Some((_, count)) = marker.split_once(':') else {
            return;
        };
        let html = browser_payload(header, count, "_sonora_metal_preview_").and_then(|html| {
            Client::browser_metadata_for(&self.name, self.requested_id.as_deref(), &html)
        });
        let Ok(mut data) = html else {
            return;
        };
        if let Some(page) = self.page.as_mut() {
            page.hide();
        }
        data.releases.sort_by(|a, b| b.date.cmp(&a.date));
        self.browser_preview = Some(marker.to_owned());
        self.candidates = data.candidates;
        self.band = data.band;
        self.requested_id = self.band.as_ref().map(|band| band.id.clone());
        self.releases = data.releases;
        self.loaded = true;
        self.loading = false;
        self.dates_loading = true;
        self.dates_error = None;
        self.via_browser = true;
        cx.notify();
    }

    /// Shows the base discography first, then applies each exact date as its request completes.
    fn check_dates(&mut self, cx: &mut Context<Self>) {
        self.cancel_dates();
        let today = jiff::Zoned::now().date();
        let releases: Vec<_> = self
            .releases
            .iter()
            .filter(|release| release.needs_date(today))
            .take(8)
            .cloned()
            .collect();
        if releases.is_empty() {
            return;
        }
        self.dates_loading = true;
        let client = self.client.clone();
        let name = self.name.clone();
        let (send, mut updates) = tokio::sync::mpsc::unbounded_channel();
        let request = self.io.spawn(async move {
            for release in releases {
                if let Some(date) = client.check_date(&name, &release).await?
                    && send.send((release.id, date)).is_err()
                {
                    break;
                }
            }
            Ok(())
        });
        self.dates_request = Some(request.abort_handle());
        self.dates_task = Some(cx.spawn(async move |this, cx| {
            while let Some((id, date)) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some(release) = this.releases.iter_mut().find(|row| row.id == id) {
                            release.date = date;
                            this.releases.sort_by(|a, b| b.date.cmp(&a.date));
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.dates_loading = false;
                this.dates_request = None;
                this.dates_task = None;
                this.dates_error = result.err().map(Into::into);
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_dates(&mut self) {
        if let Some(request) = self.dates_request.take() {
            request.abort();
        }
        self.dates_task = None;
        self.dates_loading = false;
        self.dates_error = None;
    }

    /// Returns an explicitly selected identity, including one chosen in the temporary browser.
    pub fn archives_id(&self) -> Option<&str> {
        self.requested_id.as_deref()
    }
}

/// Rejects name-cache or browser results that differ from an explicitly selected source identity.
fn require_band(data: &ArtistMetadata, expected: Option<&str>) -> anyhow::Result<()> {
    if let Some(expected) = expected
        && !data.band.as_ref().is_some_and(|band| band.id == expected)
    {
        return Err(BrowserError::WrongArtist.into());
    }
    Ok(())
}

/// Reassembles a bounded payload from the temporary browser's app-owned cookies.
fn browser_result(header: &str) -> anyhow::Result<String> {
    let count = cookie_value(header, BROWSER_DONE).ok_or(BrowserError::Incomplete)?;
    browser_payload(header, count, "_sonora_metal_")
}

/// Reads only a named app-owned capture cookie, excluding site authentication cookies.
fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// Reveals an existing hidden browser only when its script requests user interaction.
fn reveal_browser(page: &mut webview::Page, header: &str) {
    match cookie_value(header, "_sonora_metal_show") {
        Some("1") => page.show(),
        Some("0") => page.hide(),
        _ => {}
    }
}

/// Reassembles a completed or intermediate capture using separate bounded cookie namespaces.
fn browser_payload(header: &str, count: &str, prefix: &str) -> anyhow::Result<String> {
    let cookies: HashMap<_, _> = header
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .collect();
    let count: usize = count
        .parse()
        .ok()
        .filter(|count| (1..=64).contains(count))
        .ok_or(BrowserError::Incomplete)?;
    let mut encoded = String::new();
    for index in 0..count {
        let chunk = cookies
            .get(format!("{prefix}{index}").as_str())
            .ok_or(BrowserError::Incomplete)?;
        if chunk.len() > 3000 {
            return Err(BrowserError::Incomplete.into());
        }
        encoded.push_str(chunk);
    }
    let decoded = reqwest::Url::parse(&format!("https://result.invalid/?html={encoded}"))?;
    let html = decoded
        .query_pairs()
        .find(|(key, _)| key == "html")
        .map(|(_, value)| value.into_owned())
        .ok_or(BrowserError::Incomplete)?;
    Ok(html)
}

impl Drop for MetalDiscovery {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Drop for MetalArtist {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Uses an unambiguous title match from the same artist; fuzzy artwork also requires its year.
pub fn release_cover(release: &Release, albums: &[Album]) -> Option<String> {
    if let Some(album) = albums
        .iter()
        .find(|album| album_match_rank(album, release) == Some(0))
    {
        return album.cover_large.clone().or_else(|| album.cover.clone());
    }
    let mut seen = HashSet::new();
    let candidates = albums
        .iter()
        .filter(|album| album_match_rank(album, release).is_some())
        .filter(|album| seen.insert((normalize(&album.name), album.year)))
        .map(|album| (album.name.as_str(), album));
    match match_album_title(&release.title, candidates) {
        AlbumMatch::Matched(album) => album.cover_large.clone().or_else(|| album.cover.clone()),
        AlbumMatch::Missing | AlbumMatch::Ambiguous => None,
    }
}

/// Keys a resolved album by both source identities, matching resolution and artwork lookup.
fn release_key(release: &Release) -> String {
    format!("{}:{}", release.band.id, release.id)
}

/// Keeps artist and release-year checks independent from generic fuzzy title confidence.
fn album_match_rank(album: &Album, release: &Release) -> Option<u32> {
    let artist = normalize(&release.band.name);
    if normalize(&album.artists) != artist
        && !album
            .artist_refs
            .iter()
            .any(|reference| normalize(&reference.name) == artist)
    {
        return None;
    }
    let score = metal::album_title_score(&album.name, &release.title)?;
    if score == 1000 {
        return Some(0);
    }
    let year = release.date.split('-').next()?.parse::<i32>().ok()?;
    (album.year == year).then_some(1000 - score)
}

/// Falls back only for verification refusals that carry no real server-requested wait.
fn browser_required(error: &anyhow::Error) -> bool {
    error.downcast_ref::<AccessError>().is_some_and(|access| {
        access.retry_at == 0
            && matches!(
                access.reason,
                AccessReason::Challenge | AccessReason::Forbidden
            )
    })
}

/// Only the username and public API key are needed for personal album metadata.
fn lastfm_album_account(settings: &AppSettings) -> Option<(String, String)> {
    settings
        .scrobbling()
        .get("lastfm")
        .filter(|account| !account.name.is_empty() && !account.key.is_empty())
        .map(|account| (account.name.to_lowercase(), account.key.clone()))
}

fn timestamp() -> u64 {
    jiff::Timestamp::now().as_second().max(0) as u64
}
