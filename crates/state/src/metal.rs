use std::collections::HashMap;

use gpui::{Context, Entity, Task};
use jiff::civil::Date;
use metal::{ArtistStats, Band, Client, History, Release, normalize};
use music::Album;
use tokio::task::AbortHandle;

use crate::{AppSettings, Io, Origin, Session, SessionEvent, Sonora, join};

const DISCOVERIES: usize = 12;

/// The identity search and discography share one cancellable result application.
type BandDiscography = (Vec<Band>, Option<Band>, Vec<Release>);

/// Shared release discovery, listening history, and on-demand streaming album resolution.
pub struct MetalDiscovery {
    pub releases: Vec<Release>,
    pub history: History,
    pub info: HashMap<String, ArtistStats>,
    pub loading: bool,
    pub enriching: bool,
    pub error: Option<String>,
    pub history_error: Option<String>,
    pub enrichment_error: Option<String>,
    pub from: Date,
    pub to: Date,
    pub matching: Option<Release>,
    pub albums: Vec<Album>,
    pub resolving: bool,
    pub album_error: Option<String>,
    pub imported: bool,
    pub release_total: usize,
    client: Client,
    session: Entity<Session>,
    settings: Entity<AppSettings>,
    io: Io,
    loaded: bool,
    follows_today: bool,
    user: String,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
    enrichment: Option<Task<()>>,
    enrichment_request: Option<AbortHandle>,
    resolve: Option<Task<()>>,
    resolve_request: Option<AbortHandle>,
    resolved: HashMap<String, Vec<Album>>,
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
        })
        .detach();
        let to = jiff::Zoned::now().date();
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
            imported: false,
            release_total: 0,
            client: Client::new(storage::Cache::standard()),
            session,
            settings,
            io,
            loaded: false,
            follows_today: true,
            user: String::new(),
            task: None,
            request: None,
            enrichment: None,
            enrichment_request: None,
            resolve: None,
            resolve_request: None,
            resolved: HashMap::new(),
        }
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Uses cached data on revisits; changing the window never forces a cache bypass.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let today = jiff::Zoned::now().date();
        if self.follows_today && self.to != today {
            self.cancel_load();
            self.to = today;
            self.from = today
                .checked_sub(jiff::Span::new().days(6))
                .expect("recent date is representable");
            self.releases.clear();
            self.release_total = 0;
            self.imported = false;
            self.loaded = false;
        }
        if self.loading || self.loaded {
            return;
        }
        self.loading = true;
        self.error = None;
        self.history_error = None;
        let from = self.from.to_string();
        let to = self.to.to_string();
        let client = self.client.clone();
        let account = self.settings.read(cx).scrobbling().get("lastfm").cloned();
        self.user = account
            .as_ref()
            .map(|account| account.name.clone())
            .unwrap_or_default();
        let request = self.io.spawn(async move {
            let history = async {
                match account.filter(|account| !account.name.is_empty() && !account.key.is_empty())
                {
                    Some(account) => {
                        let seed = dirs::home_dir()
                            .map(|home| home.join(".hermes/cache/lastfm_artists.json"));
                        client.history(&account.name, &account.key, seed).await
                    }
                    None => Ok(History::default()),
                }
            };
            let (releases, history) = tokio::join!(client.releases(&from, &to), history);
            Ok((releases, history))
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.request = None;
                this.task = None;
                match loaded {
                    Ok((releases, history)) => {
                        match releases {
                            Ok(feed) => {
                                this.releases = feed.releases;
                                this.release_total = feed.total;
                                this.imported = feed.imported;
                                this.loaded = true;
                            }
                            Err(error) => this.error = Some(format!("{error:#}")),
                        }
                        match history {
                            Ok(history) => this.history = history,
                            Err(error) => this.history_error = Some(format!("{error:#}")),
                        }
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Changes the date window without bypassing cached responses or the shared request gate.
    pub fn shift(&mut self, days: i64, cx: &mut Context<Self>) {
        self.imported = false;
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
        self.loaded = false;
        self.load(cx);
    }

    /// Returns to the seven-day window ending today in the user's local timezone.
    pub fn today(&mut self, cx: &mut Context<Self>) {
        self.follows_today = true;
        self.imported = false;
        self.release_total = 0;
        self.cancel_load();
        self.to = jiff::Zoned::now().date();
        self.from = self
            .to
            .checked_sub(jiff::Span::new().days(6))
            .expect("recent date is representable");
        self.loaded = false;
        self.releases.clear();
        self.load(cx);
    }

    /// Retries via the normal cache and request gate; this never overrides a cooldown.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.loaded = false;
        self.load(cx);
    }

    /// Reads saved release data, allowing existing daily fetches to be reused during a cooldown.
    pub fn import(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.cancel_load();
        self.loading = true;
        self.error = None;
        let client = self.client.clone();
        let from = self.from.to_string();
        let to = self.to.to_string();
        let request = self.io.spawn(async move {
            if std::fs::metadata(&path)?.len() > 16 * 1024 * 1024 {
                anyhow::bail!("saved release response exceeds 16 MiB");
            }
            let text = std::fs::read_to_string(path)?;
            client.import_releases(&from, &to, &text).await
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.task = None;
                this.request = None;
                match loaded {
                    Ok(feed) => {
                        this.releases = feed.releases;
                        this.release_total = feed.total;
                        this.loaded = true;
                        this.imported = true;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn playcount(&self, band: &str) -> u64 {
        self.history
            .artists
            .get(&normalize(band))
            .map(|artist| artist.playcount)
            .unwrap_or_default()
    }

    /// Enriches a bounded batch on demand, never a request per release or per render.
    pub fn enrich(&mut self, cx: &mut Context<Self>) {
        if self.enriching {
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
        for release in &self.releases {
            let key = normalize(&release.band.name);
            if release.kind == "Full-length"
                && self.playcount(&release.band.name) <= 5
                && !self.info.contains_key(&key)
                && !bands.contains(&release.band.name)
            {
                bands.push(release.band.name.clone());
            }
            if bands.len() == DISCOVERIES {
                break;
            }
        }
        if bands.is_empty() {
            return;
        }
        self.enriching = true;
        self.enrichment_error = None;
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let mut info = Vec::new();
            let mut failure = None;
            for band in bands {
                match client.artist_info(&band, &account.key).await {
                    Ok(artist) => info.push((normalize(&band), artist)),
                    Err(error) => {
                        failure = Some(format!("{error:#}"));
                        break;
                    }
                }
            }
            Ok((info, failure))
        });
        self.enrichment_request = Some(request.abort_handle());
        self.enrichment = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.enriching = false;
                this.enrichment_request = None;
                this.enrichment = None;
                match loaded {
                    Ok((info, error)) => {
                        this.info.extend(info);
                        this.enrichment_error = error;
                    }
                    Err(error) => this.enrichment_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Searches only when a listener requests a release. Results remain account-scoped in memory.
    pub fn resolve(&mut self, release: Release, cx: &mut Context<Self>) {
        self.dismiss_album(cx);
        self.matching = Some(release.clone());
        let key = format!("{}:{}", release.band.id, release.id);
        if let Some(albums) = self.resolved.get(&key) {
            self.albums = albums.clone();
            cx.notify();
            return;
        }
        let Some(provider) = self.session.read(cx).client() else {
            self.album_error = Some(i18n::lookup("metal-sign-in", None).to_string());
            cx.notify();
            return;
        };
        self.resolving = true;
        let request = self.io.spawn(async move {
            let mut albums = provider
                .search_albums(&format!("{} {}", release.band.name, release.title))
                .await?;
            albums.sort_by_key(|album| !album_matches(album, &release));
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
                    Err(error) => this.album_error = Some(format!("{error:#}")),
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

    fn cancel_load(&mut self) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
        if let Some(request) = self.enrichment_request.take() {
            request.abort();
        }
        self.task = None;
        self.enrichment = None;
        self.loading = false;
        self.enriching = false;
    }

    fn cancel(&mut self) {
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

/// An artist's lazily fetched Archives identity and discography, separate from provider data.
pub struct MetalArtist {
    pub name: String,
    pub candidates: Vec<Band>,
    pub band: Option<Band>,
    pub releases: Vec<Release>,
    pub loading: bool,
    pub error: Option<String>,
    client: Client,
    io: Io,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
}

impl MetalArtist {
    /// Shares the app's paced metadata client and cancels its loads on provider sign-out.
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
            error: None,
            client,
            io,
            task: None,
            request: None,
        }
    }

    /// Searches once per name and automatically loads only a single exact band identity.
    pub fn open(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.name == name {
            return;
        }
        self.clear();
        self.name = name.to_owned();
        if name.is_empty() {
            cx.notify();
            return;
        }
        self.loading = true;
        let name = name.to_owned();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let candidates = client.bands(&name).await?;
            let exact: Vec<_> = candidates
                .iter()
                .filter(|band| normalize(&band.name) == normalize(&name))
                .collect();
            match exact.as_slice() {
                [band] => Ok((
                    candidates.clone(),
                    Some((*band).clone()),
                    client.discography(band).await?,
                )),
                _ => Ok((candidates, None, Vec::new())),
            }
        });
        self.receive(request, cx);
    }

    pub fn choose(&mut self, band: Band, cx: &mut Context<Self>) {
        self.clear();
        self.band = Some(band.clone());
        self.loading = true;
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let releases = client.discography(&band).await?;
            Ok((Vec::new(), Some(band), releases))
        });
        self.receive(request, cx);
    }

    /// Reopens the cached identity list so failures and mistaken namesake choices can be corrected.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let name = std::mem::take(&mut self.name);
        self.open(&name, cx);
    }

    fn receive(
        &mut self,
        request: tokio::task::JoinHandle<anyhow::Result<BandDiscography>>,
        cx: &mut Context<Self>,
    ) {
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.request = None;
                this.task = None;
                match loaded {
                    Ok((candidates, band, mut releases)) => {
                        releases.sort_by(|a, b| b.date.cmp(&a.date));
                        this.candidates = candidates;
                        this.band = band;
                        this.releases = releases;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn clear(&mut self) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
        self.task = None;
        self.candidates.clear();
        self.band = None;
        self.releases.clear();
        self.loading = false;
        self.error = None;
    }
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

fn album_matches(album: &Album, release: &Release) -> bool {
    normalize(&album.name) == normalize(&release.title)
        && (normalize(&album.artists) == normalize(&release.band.name)
            || album
                .artist_refs
                .iter()
                .any(|artist| normalize(&artist.name) == normalize(&release.band.name)))
}
