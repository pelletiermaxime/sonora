use std::collections::HashMap;

use gpui::{Context, Entity, EventEmitter, Task};
use metal::{album_search_title, album_title_score, normalize};
use music::{Album, ArtistRef};
use tokio::task::AbortHandle;

use crate::{Io, Session, SessionEvent, join};

/// Names from external charts need provider identities before opening a Sonora page.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CatalogQuery {
    Artist(String),
    Album { artist: String, title: String },
}

/// Provider results retain their identity and display metadata for an explicit choice.
#[derive(Clone)]
pub enum CatalogCandidate {
    Artist(ArtistRef),
    Album(Album),
}

/// A resolved identity is consumed by the view's router, without coupling state to routing.
pub enum CatalogOpened {
    Artist(String),
    Album(String),
}

/// Resolves chart links on demand and reuses successful searches until the session changes.
pub struct CatalogLookup {
    pub query: Option<CatalogQuery>,
    pub candidates: Vec<CatalogCandidate>,
    pub loading: bool,
    pub error: Option<&'static str>,
    session: Entity<Session>,
    cache: HashMap<CatalogQuery, Vec<CatalogCandidate>>,
    task: Option<Task<()>>,
    request: Option<AbortHandle>,
}

impl EventEmitter<CatalogOpened> for CatalogLookup {}

impl CatalogLookup {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&session, |this, _, event, cx| {
            if matches!(event, SessionEvent::SignedOut) {
                this.dismiss(cx);
                this.cache.clear();
            }
        })
        .detach();
        Self {
            query: None,
            candidates: Vec::new(),
            loading: false,
            error: None,
            session,
            cache: HashMap::new(),
            task: None,
            request: None,
        }
    }

    /// Opens a single exact result immediately; namesakes and alternate editions need a choice.
    pub fn open(&mut self, query: CatalogQuery, cx: &mut Context<Self>) {
        self.dismiss(cx);
        self.query = Some(query.clone());
        let Some(client) = self.session.read(cx).client() else {
            self.error = Some("metal-sign-in");
            cx.notify();
            return;
        };
        if let Some(candidates) = self.cache.get(&query).cloned() {
            self.apply(candidates, cx);
            return;
        }
        self.loading = true;
        let request = Io::global(cx).spawn(async move {
            match &query {
                CatalogQuery::Artist(name) => {
                    crate::empath::provider_artists(client.as_ref(), name)
                        .await
                        .map(|artists| {
                            artists
                                .into_iter()
                                .map(CatalogCandidate::Artist)
                                .collect::<Vec<_>>()
                        })
                }
                CatalogQuery::Album { artist, title } => {
                    let mut albums = client.search_albums(&format!("{artist} {title}")).await?;
                    if !albums
                        .iter()
                        .any(|album| album_score(album, artist, title).is_some())
                        && album_search_title(title) != title
                    {
                        albums.extend(
                            client
                                .search_albums(&format!("{artist} {}", album_search_title(title)))
                                .await?,
                        );
                    }
                    albums.retain(|album| album_score(album, artist, title).is_some());
                    albums
                        .sort_by_key(|album| std::cmp::Reverse(album_score(album, artist, title)));
                    let mut ids = std::collections::HashSet::new();
                    albums.retain(|album| ids.insert(album.id.clone()));
                    Ok(albums.into_iter().map(CatalogCandidate::Album).collect())
                }
            }
        });
        self.request = Some(request.abort_handle());
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.request = None;
                this.task = None;
                match result {
                    Ok(candidates) => {
                        if !candidates.is_empty()
                            && let Some(query) = this.query.clone()
                        {
                            this.cache.insert(query, candidates.clone());
                        }
                        this.apply(candidates, cx);
                    }
                    Err(error) => {
                        log::warn!("catalog: cannot resolve chart link: {error:#}");
                        this.error = Some("listening-lookup-failed");
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn apply(&mut self, candidates: Vec<CatalogCandidate>, cx: &mut Context<Self>) {
        self.candidates = candidates;
        let exact = match (self.query.as_ref(), self.candidates.as_slice()) {
            (Some(CatalogQuery::Artist(_)), [CatalogCandidate::Artist(_)]) => true,
            (Some(CatalogQuery::Album { artist, title }), [CatalogCandidate::Album(album)]) => {
                album_score(album, artist, title) == Some(1000)
            }
            _ => false,
        };
        if exact {
            self.choose(0, cx);
        }
        cx.notify();
    }

    /// Selects only a result belonging to the current request.
    pub fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(candidate) = self.candidates.get(index) else {
            return;
        };
        let opened = match candidate {
            CatalogCandidate::Artist(artist) => {
                let Some(id) = artist.id.clone() else { return };
                CatalogOpened::Artist(id)
            }
            CatalogCandidate::Album(album) => CatalogOpened::Album(album.id.clone()),
        };
        self.dismiss(cx);
        cx.emit(opened);
    }

    /// Closing a chooser cancels its provider search as well as its UI task.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
        self.task = None;
        self.query = None;
        self.candidates.clear();
        self.loading = false;
        self.error = None;
        cx.notify();
    }
}

impl Drop for CatalogLookup {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
    }
}

/// Restricts fuzzy titles to the requested artist; only exact titles open automatically.
fn album_score(album: &Album, artist: &str, title: &str) -> Option<u32> {
    let artist = normalize(artist);
    if normalize(&album.artists) != artist
        && !album
            .artist_refs
            .iter()
            .any(|reference| normalize(&reference.name) == artist)
    {
        return None;
    }
    album_title_score(&album.name, title)
}
