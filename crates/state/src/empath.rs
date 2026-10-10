use gpui::{Context, Entity, Task};
use metal::{EmpathBand, EmpathCached, EmpathClient, EmpathGenres, EmpathSimilar, normalize};
use music::ArtistRef;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::task::AbortHandle;

use crate::{Io, Session, SessionEvent, join};

const SIMILAR_RETRY_DELAY: Duration = Duration::from_secs(60);

/// Cached genre rankings and similar bands, with provider searches only after a band is clicked.
pub struct Empath {
    pub genres: Option<EmpathGenres>,
    pub genres_loading: bool,
    pub genres_failed: bool,
    pub genres_stale: bool,
    pub similar: Option<EmpathSimilar>,
    pub similar_loading: bool,
    pub similar_failed: bool,
    pub similar_stale: bool,
    pub identities: Vec<EmpathBand>,
    pub name: String,
    pub matching: Option<EmpathBand>,
    pub matches: Vec<ArtistRef>,
    pub matching_loading: bool,
    pub matching_failed: bool,
    client: EmpathClient,
    session: Entity<Session>,
    io: Io,
    identity: Option<String>,
    similar_fresh_until: u64,
    similar_retry_after: Option<Instant>,
    genres_task: Option<Task<()>>,
    genres_request: Option<AbortHandle>,
    similar_task: Option<Task<()>>,
    similar_request: Option<AbortHandle>,
    matching_task: Option<Task<()>>,
    matching_request: Option<AbortHandle>,
    provider_bands: HashMap<String, EmpathBand>,
}

impl Empath {
    pub fn new(session: Entity<Session>, io: Io, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&session, |this, _, event, cx| {
            if matches!(event, SessionEvent::SignedOut) {
                this.cancel();
                this.genres = None;
                this.similar = None;
                this.identities.clear();
                this.name.clear();
                this.identity = None;
                this.similar_fresh_until = 0;
                this.similar_retry_after = None;
                this.provider_bands.clear();
                cx.notify();
            }
        })
        .detach();
        Self {
            genres: None,
            genres_loading: false,
            genres_failed: false,
            genres_stale: false,
            similar: None,
            similar_loading: false,
            similar_failed: false,
            similar_stale: false,
            identities: Vec::new(),
            name: String::new(),
            identity: None,
            similar_fresh_until: 0,
            similar_retry_after: None,
            matching: None,
            matches: Vec::new(),
            matching_loading: false,
            matching_failed: false,
            client: EmpathClient::new(storage::Cache::standard()),
            session,
            io,
            genres_task: None,
            genres_request: None,
            similar_task: None,
            similar_request: None,
            matching_task: None,
            matching_request: None,
            provider_bands: HashMap::new(),
        }
    }

    /// Refresh revalidates the source; normal navigation uses the persistent 24-hour cache.
    pub fn load_genres(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.genres_loading {
            return;
        }
        self.genres_loading = true;
        self.genres_failed = false;
        let client = self.client.clone();
        let request = self.io.spawn(async move { client.genres(refresh).await });
        self.genres_request = Some(request.abort_handle());
        self.genres_task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.genres_loading = false;
                this.genres_task = None;
                this.genres_request = None;
                match result {
                    Ok(EmpathCached { data, expired, .. }) => {
                        this.genres = Some(data);
                        this.genres_stale = expired;
                    }
                    Err(error) => {
                        log::warn!("empath: cannot load genres: {error:#}");
                        this.genres_failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// An Archives selection wins over name matching. Ambiguous names remain explicit choices.
    pub fn load_similar(
        &mut self,
        name: &str,
        id: Option<String>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        let same = self.name == name && self.identity == id;
        if !refresh
            && same
            && (self.similar_loading
                || (jiff::Timestamp::now().as_second().max(0) as u64) < self.similar_fresh_until
                || self
                    .similar_retry_after
                    .is_some_and(|deadline| Instant::now() < deadline))
        {
            return;
        }
        if let Some(request) = self.similar_request.take() {
            request.abort();
        }
        self.similar_task = None;
        self.name = name.to_owned();
        self.identity = id.clone();
        self.similar_fresh_until = 0;
        self.similar_retry_after = None;
        if !same {
            self.similar = None;
            self.identities.clear();
        }
        self.similar_failed = false;
        self.similar_stale = false;
        self.similar_loading = !name.is_empty();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let asked = name.to_owned();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let identities = client.identities(&asked, id.as_deref()).await?;
            let similar = match identities.data.as_slice() {
                [band] => Some(client.similar(&band.id, refresh).await?),
                _ => None,
            };
            Ok((identities, similar))
        });
        self.similar_request = Some(request.abort_handle());
        self.similar_task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.similar_loading = false;
                this.similar_request = None;
                this.similar_task = None;
                match result {
                    Ok((identities, data)) => {
                        this.identities = identities.data;
                        this.similar_fresh_until = identities.fresh_until;
                        this.similar_stale = identities.expired;
                        this.similar = None;
                        if let Some(loaded) = data {
                            this.similar = Some(loaded.data);
                            this.similar_stale |= loaded.expired;
                            this.similar_fresh_until =
                                this.similar_fresh_until.min(loaded.fresh_until);
                        }
                        if this.similar_stale {
                            this.similar_retry_after = Some(Instant::now() + SIMILAR_RETRY_DELAY);
                        }
                    }
                    Err(error) => {
                        log::warn!("empath: cannot load similar bands: {error:#}");
                        this.similar_failed = true;
                        this.similar_retry_after = Some(Instant::now() + SIMILAR_RETRY_DELAY);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Searches the active provider and lets the listener choose among exact name matches.
    pub fn find_band(&mut self, band: EmpathBand, cx: &mut Context<Self>) {
        self.dismiss_match(cx);
        self.matching = Some(band.clone());
        let Some(client) = self.session.read(cx).client() else {
            cx.notify();
            return;
        };
        self.matching_loading = true;
        let request = self
            .io
            .spawn(async move { provider_artists(client.as_ref(), &band.name).await });
        self.matching_request = Some(request.abort_handle());
        self.matching_task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.matching_loading = false;
                this.matching_request = None;
                this.matching_task = None;
                match result {
                    Ok(matches) => {
                        this.matches = matches;
                    }
                    Err(error) => {
                        log::warn!("empath: cannot find provider band: {error:#}");
                        this.matching_failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Carries the selected Archives identity onto the provider page chosen by the listener.
    pub fn choose_match(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if !self
            .matches
            .iter()
            .any(|artist| artist.id.as_deref() == Some(id))
        {
            return false;
        }
        if let Some(band) = self.matching.clone() {
            self.provider_bands.insert(id.to_owned(), band);
        }
        self.dismiss_match(cx);
        true
    }

    /// A provider match is account-scoped and reused only for the same band name.
    pub fn archives_id(&self, provider_id: &str, name: &str) -> Option<String> {
        self.provider_bands
            .get(provider_id)
            .filter(|band| normalize(&band.name) == normalize(name))
            .map(|band| band.id.clone())
    }

    pub fn dismiss_match(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.matching_request.take() {
            request.abort();
        }
        self.matching_task = None;
        self.matching = None;
        self.matches.clear();
        self.matching_loading = false;
        self.matching_failed = false;
        cx.notify();
    }

    /// Remembers an explicit Archives selection for this provider artist, including namesakes.
    pub fn remember_archives(
        &mut self,
        provider_id: &str,
        band: EmpathBand,
        cx: &mut Context<Self>,
    ) {
        if self
            .provider_bands
            .get(provider_id)
            .is_some_and(|saved| saved.id == band.id && saved.name == band.name)
        {
            return;
        }
        self.provider_bands.insert(provider_id.to_owned(), band);
        cx.notify();
    }

    /// Releases a source identity when the listener explicitly asks to choose another band.
    pub fn forget_archives(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        if self.provider_bands.remove(provider_id).is_some() {
            cx.notify();
        }
    }

    fn cancel(&mut self) {
        for request in [
            self.genres_request.take(),
            self.similar_request.take(),
            self.matching_request.take(),
        ]
        .into_iter()
        .flatten()
        {
            request.abort();
        }
        self.genres_task = None;
        self.similar_task = None;
        self.matching_task = None;
        self.genres_loading = false;
        self.similar_loading = false;
        self.matching_loading = false;
        self.matches.clear();
        self.matching = None;
    }
}

/// Both external charts use provider credits to find exact names without guessing artist IDs.
pub(crate) async fn provider_artists(
    client: &dyn music::MusicApi,
    name: &str,
) -> anyhow::Result<Vec<ArtistRef>> {
    let (albums, tracks) = tokio::join!(client.search_albums(name), client.search(name));
    let mut refs = Vec::new();
    let mut success = false;
    match albums {
        Ok(albums) => {
            success = true;
            refs.extend(albums.into_iter().flat_map(|album| album.artist_refs));
        }
        Err(error) => log::warn!("catalog: cannot search provider albums: {error:#}"),
    }
    match tracks {
        Ok(tracks) => {
            success = true;
            refs.extend(tracks.into_iter().flat_map(|track| track.artist_refs));
        }
        Err(error) => log::warn!("catalog: cannot search provider tracks: {error:#}"),
    }
    anyhow::ensure!(success, "cannot search provider artists");
    let name = normalize(name);
    let mut ids = std::collections::HashSet::new();
    refs.retain(|artist| {
        normalize(&artist.name) == name
            && artist.id.as_ref().is_some_and(|id| ids.insert(id.clone()))
    });
    Ok(refs)
}

impl Drop for Empath {
    fn drop(&mut self) {
        self.cancel();
    }
}
