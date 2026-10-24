use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Pixels, Point, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, WeakEntity, Window, div, point, px,
};

use crate::chrome::Chrome;
use crate::chrome::metal::DiscographyTab;
use crate::shared::cells;
use i18n::t;
use music::{SavedArtist, Track};
use state::{AppSettings, ArtistDetail, Io, MetalArtist, Origin, Playback, Sonora};
use ui::ActiveTheme as _;
use ui::Listing as _;
use ui::{
    Button, Card, Deck, MIN_CONTENT, Mode, Notice, Pending, Picker, Pin, PinKind, Popovers, Popup,
    Scrollbar, Scroller, TabBar, TableDelegate, TableEvent, TableState, Text, scrolled, snapped,
    table,
};

use crate::chrome::tools;
use crate::chrome::{Toolbar, Tooled};
use crate::shared::about::{AboutArtist, about_modal};
use crate::shared::album_grid::CardGrid;
use crate::shared::cards::{self, ReleaseFilter};
use crate::shared::confirm::Confirm;
use crate::shared::hero::{HeroMetaStrip, HeroPlayButton, PageHero};
use crate::shared::menus::{ItemMenu, artist_menu};
use crate::shared::page;
use crate::shared::picks::{Picks, Shape};
use crate::shared::shelves::{Rail, RailSpec};
use crate::shared::tracks::{PlaybackStatus, TrackSource, Tracks, drop_picked, playback_status};
use crate::shared::trouble;

const SECTION: &str = "artist";
const RELEASE_ROWS: usize = 2;
const LISTED: usize = 5;
const LISTED_MAX: usize = 10;

struct ArtistTracks {
    detail: Entity<ArtistDetail>,
    shown: Rc<Cell<usize>>,
}

impl Tracks for ArtistTracks {
    fn tracks<'a>(&self, cx: &'a App) -> &'a [Track] {
        let tracks = self.detail.read(cx).tracks();
        &tracks[..self.shown.get().min(tracks.len())]
    }

    fn is_loading(&self, cx: &App) -> bool {
        self.detail.read(cx).is_loading()
    }

    /// As many rows as the list shows before it is expanded.
    fn pending(&self, cx: &App) -> Option<Pending> {
        self.is_loading(cx).then_some(Pending::Rows(LISTED))
    }
}

pub(crate) struct ArtistView {
    detail: Entity<ArtistDetail>,
    metal: Entity<MetalArtist>,
    metal_tab: DiscographyTab,
    similar_expanded: bool,
    playback: Entity<Playback>,
    playback_status: PlaybackStatus,
    artist_id: Option<String>,
    release_filter: ReleaseFilter,
    releases_expanded: bool,
    release_padding: Pixels,
    release_padding_offset: Pixels,
    width: Pixels,
    scrollbar: Entity<Scrollbar>,
    about_bar: Entity<Scrollbar>,
    about_open: bool,
    table: Entity<TableState<TrackSource>>,
    shown: Rc<Cell<usize>>,
    mode: Mode,
    popular: Rc<Vec<Track>>,
    popular_page: usize,
    popular_columns: usize,
    track_menu: ItemMenu,
    track_context: Option<(usize, Point<Pixels>)>,
    settings: Entity<AppSettings>,
    toolbar: Entity<Toolbar>,
    me: WeakEntity<Self>,
    popovers: Popovers,
    /// Where the releases grid's leading edge landed last frame, in window coordinates, or
    /// none until it has been laid out once. `hold_releases` needs it to tell how deep the
    /// page is scrolled into the grid.
    release_lead: Rc<Cell<Option<Pixels>>>,
    /// The packing the grid was laid out with last frame, so a change of width can hold its
    /// place rather than slide the cards under the user.
    release_columns: usize,
    release_tile: Pixels,
    /// The recommendation rails under the releases, in page order: appears-on.
    rails: Vec<Rail>,
}

impl ArtistView {
    pub(crate) fn new(
        detail: Entity<ArtistDetail>,
        playback: Entity<Playback>,
        cx: &mut Context<Self>,
    ) -> Self {
        let width = MIN_CONTENT;
        let id = cx.entity_id();
        let scrollbar = cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id));
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(id));
        let settings = Sonora::global(cx).settings.clone();
        let client = Sonora::global(cx).metal.read(cx).client();
        let metal = cx.new(|cx| MetalArtist::new(client, Io::global(cx), cx));
        cx.observe(&metal, |this, metal, cx| {
            let metal = metal.read(cx);
            if !metal.loading {
                if let Some(band) = metal.band.as_ref()
                    && metal.archives_id() == Some(band.id.as_str())
                    && let Some(provider_id) = this.detail.read(cx).id().map(str::to_owned)
                {
                    let band = metal::EmpathBand {
                        id: band.id.clone(),
                        name: band.name.clone(),
                    };
                    Sonora::global(cx).empath.clone().update(cx, |state, cx| {
                        state.remember_archives(&provider_id, band, cx);
                    });
                }
                this.sync_similar(cx);
            }
            cx.notify();
        })
        .detach();
        let empath = Sonora::global(cx).empath.clone();
        cx.observe(&empath, |_, _, cx| cx.notify()).detach();
        let discovery = Sonora::global(cx).metal.clone();
        cx.observe(&discovery, |_, _, cx| cx.notify()).detach();
        let saved = settings.read(cx).table(SECTION);
        let sorting = settings.read(cx).sorting(SECTION);
        let mode = settings.read(cx).view_or(SECTION, Mode::List);
        let columns = crate::shared::tracks::artist_columns(
            Sonora::global(cx)
                .session
                .read(cx)
                .capabilities()
                .playcounts,
        );
        let scroll = scrollbar.read(cx).scroll().clone();
        let shown = Rc::new(Cell::new(LISTED));
        let table = cx.new(|cx| {
            let menu_scrollbar = cx.new(|_| Scrollbar::inset().watching(id));
            crate::shared::tracks::follow_downloads(cx);
            let source = TrackSource::new(
                columns,
                ArtistTracks {
                    detail: detail.clone(),
                    shown: shown.clone(),
                },
                playback.clone(),
                menu_scrollbar,
                cx,
            )
            .from({
                let detail = detail.clone();
                move |cx: &App| {
                    let detail = detail.read(cx);
                    let name = detail.artist()?.name.clone();
                    Some(Origin::artist(detail.id()?).named(name))
                }
            })
            .with_liked(Sonora::global(cx).library.clone());
            let source = source.table(cx.weak_entity());
            let mut delegate = TableDelegate::new(source, width, cx);
            delegate.set_layout(saved, cx);
            delegate.set_sorting(sorting.flatten(), cx);
            TableState::new(delegate, cx).follow(scroll)
        });

        cx.subscribe(&detail, |_, _, event, cx| {
            let state::ArtistDetailEvent::Gone(id) = event;
            if router::trail(cx).read(cx).current()
                == router::Destination::Artist(id.clone().into())
            {
                router::replace(router::Destination::Local(router::LibraryTab::Artists), cx);
            }
        })
        .detach();

        cx.observe(&detail, |this, detail, cx| {
            let name = detail
                .read(cx)
                .artist()
                .map(|artist| artist.name.clone())
                .unwrap_or_default();
            let empath = Sonora::global(cx).empath.clone();
            let id = detail
                .read(cx)
                .id()
                .and_then(|provider_id| empath.read(cx).archives_id(provider_id, &name));
            this.metal.update(cx, |metal, cx| metal.open(&name, id, cx));
            this.sync_similar(cx);
            let artist_id = detail.read(cx).id().map(str::to_owned);
            if this.artist_id != artist_id {
                this.artist_id = artist_id;
                this.release_filter = ReleaseFilter::All;
                this.metal_tab = DiscographyTab::Main;
                this.similar_expanded = false;
                this.releases_expanded = false;
                this.release_padding = Pixels::ZERO;
                this.release_padding_offset = Pixels::ZERO;
                this.about_open = false;
                this.rails.clear();
                this.shown.set(LISTED);
                this.scrollbar.update(cx, |bar, cx| {
                    bar.set_max_offset(None, cx);
                    bar.scroll().set_offset(gpui::Point::default());
                });
            }
            this.popular = Rc::new(detail.read(cx).tracks().to_vec());
            this.popular_page = 0;
            this.track_menu.reset(cx);
            this.track_context = None;
            this.rebuild(cx);
            cx.notify();
        })
        .detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let library = Sonora::global(cx).library.clone();
        cx.observe(&library, |this, _, cx| {
            this.table.update(cx, |table, cx| table.refresh(cx));
            cx.notify();
        })
        .detach();
        let current_playback = playback_status(&playback, cx);
        let artist_id = detail.read(cx).id().map(str::to_owned);
        cx.observe(&playback, |this, playback, cx| {
            let current = playback_status(&playback, cx);
            if this.playback_status == current {
                return;
            }
            this.playback_status = current;
            this.table.update(cx, |table, cx| table.refresh(cx));
            cx.notify();
        })
        .detach();
        cx.subscribe(&table, |this, _, event, cx| match event {
            TableEvent::DoubleClicked(display) => {
                page::play(&this.table, &this.playback, *display, cx)
            }
            TableEvent::Activated(display) => {
                page::play_or_toggle(&this.table, &this.playback, *display, cx)
            }
            TableEvent::Removed => drop_picked(&this.table, cx),
            _ => this.persist(cx),
        })
        .detach();

        let me = cx.entity();
        let toolbar = Toolbar::tooled(&me, cx);

        Self {
            popular: Rc::new(detail.read(cx).tracks().to_vec()),
            detail,
            metal,
            metal_tab: DiscographyTab::Main,
            similar_expanded: false,
            playback,
            playback_status: current_playback,
            artist_id,
            release_filter: ReleaseFilter::All,
            releases_expanded: false,
            release_padding: Pixels::ZERO,
            release_padding_offset: Pixels::ZERO,
            width,
            scrollbar,
            about_bar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            about_open: false,
            table,
            shown,
            mode,
            popular_page: 0,
            popular_columns: 0,
            track_menu: ItemMenu::new(playlist_scrollbar, cx),
            track_context: None,
            settings,
            toolbar,
            me: me.downgrade(),
            popovers: Popovers::default(),
            release_lead: Rc::new(Cell::new(None)),
            release_columns: 0,
            release_tile: Pixels::ZERO,
            rails: Vec::new(),
        }
    }

    /// Synchronizes source identity on provider navigation as well as Archives data updates.
    fn sync_similar(&mut self, cx: &mut Context<Self>) {
        let detail = self.detail.read(cx);
        let name = detail
            .artist()
            .map(|artist| artist.name.clone())
            .unwrap_or_default();
        let empath = Sonora::global(cx).empath.clone();
        let known = detail
            .id()
            .and_then(|provider_id| empath.read(cx).archives_id(provider_id, &name));
        let metal = self.metal.read(cx);
        if metal.loading && known.is_none() {
            return;
        }
        let id = known.or_else(|| {
            if metal.name != name {
                return None;
            }
            metal.band.as_ref().map(|band| band.id.clone())
        });
        empath.update(cx, |state, cx| state.load_similar(&name, id, false, cx));
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        page::store(
            &self.settings.clone(),
            &self.table.clone(),
            SECTION,
            SECTION,
            cx,
        );
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.rebuild(cx);
        });
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        if mode == Mode::List {
            self.table.clone().set_width(self.width, cx);
        }
        self.track_context = None;
        self.settings
            .update(cx, |settings, cx| settings.set_view(SECTION, mode, cx));
        cx.notify();
    }

    fn header(&self, cx: &Context<Self>) -> AnyElement {
        let artist = self.detail.read(cx).artist();
        let title = artist
            .map(|artist| SharedString::from(artist.name.clone()))
            .unwrap_or_default();
        let listeners = artist
            .and_then(|artist| artist.monthly_listeners)
            .map(|count| {
                let value = cells::count(count);
                t!("artist-monthly-listeners", count = count, value = &value)
            });
        let discovery = Sonora::global(cx).metal.read(cx);
        let plays = artist
            .filter(|_| !discovery.history.source.is_empty())
            .map(|artist| discovery.playcount(&artist.name));
        let genre = self
            .metal
            .read(cx)
            .band
            .as_ref()
            .filter(|band| {
                artist.is_some_and(|artist| {
                    metal::normalize(&band.name) == metal::normalize(&artist.name)
                })
            })
            .map(|band| band.genre.clone())
            .filter(|genre| !genre.is_empty());
        let has_meta = listeners.is_some() || plays.is_some() || genre.is_some();
        let meta = HeroMetaStrip::new()
            .when_some(listeners, |meta, listeners| meta.text(listeners))
            .when_some(plays, |meta, plays| {
                meta.text(t!("metal-personal-plays", count = plays))
            })
            .when_some(genre, |meta, genre| meta.text(genre));
        let overflow = self.saved_artist(cx).map(|artist| {
            Picker::icon("artist-overflow", &self.popovers, "icons/ellipsis.svg")
                .tooltip("common-more")
                .large()
                .left()
                .menu(artist_menu(artist, self.playback.clone(), true, cx))
        });
        let actions = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                HeroPlayButton::new(
                    "play-artist",
                    t!("artist-play"),
                    self.popular.as_ref().clone(),
                    self.playback.clone(),
                )
                .from(self.playing_from(cx)),
            )
            .child(
                HeroPlayButton::shuffle(
                    "shuffle-artist",
                    self.popular.as_ref().clone(),
                    self.playback.clone(),
                )
                .from(self.playing_from(cx)),
            )
            .children(self.favorite_button(cx))
            .children(overflow);

        let cover = artist.and_then(|artist| artist.cover_large.clone());
        let pin = self
            .detail
            .read(cx)
            .id()
            .map(|id| Pin::new(PinKind::Artist, id, title.clone()).cover(cover.clone()));

        PageHero::new("artist-hero", title)
            .pending(artist.is_none() && self.detail.read(cx).is_loading())
            .pin(pin)
            .cover(cover)
            .eyebrow(t!("artist-eyebrow"))
            .when(has_meta, |hero| hero.meta(meta))
            .actions(actions)
            .circle()
            .into_any_element()
    }

    fn saved_artist(&self, cx: &App) -> Option<SavedArtist> {
        let detail = self.detail.read(cx);
        let artist = detail.artist()?;

        Some(SavedArtist {
            id: detail.id()?.to_owned(),
            name: artist.name.clone(),
            cover: artist.cover_large.clone(),
            added_at: None,
        })
    }

    fn favorite_button(&self, cx: &App) -> Option<Button> {
        let theme = *cx.theme();
        // A provider with no followed artists has nothing for this to toggle.
        if !Sonora::global(cx)
            .session
            .read(cx)
            .capabilities()
            .follow_artists
        {
            return None;
        }
        let library = Sonora::global(cx).library.clone();
        let target = self.saved_artist(cx)?;
        let saved = library.read(cx).saved_artist(&target.id);

        let heart = Button::new("artist-toggle-library")
            .outline()
            .icon(match saved {
                true => "icons/heart-filled.svg",
                false => "icons/heart.svg",
            })
            .tooltip(match saved {
                true => "menu-remove-from-library",
                false => "menu-add-to-library",
            })
            .disabled(library.read(cx).pending_artist(&target.id));

        Some(
            match saved {
                true => heart.tint(theme.primary),
                false => heart,
            }
            .on_click(move |_, _, cx| {
                match saved && !Confirm::unstarring(&target.id, cx) {
                    true => Confirm::artists(vec![target.clone()], cx),
                    false => {
                        library.update(cx, |library, cx| library.heart_artist(target.clone(), cx));
                    }
                }
            }),
        )
    }

    fn release_height(
        &self,
        filter: ReleaseFilter,
        expanded: bool,
        columns: usize,
        window: &Window,
        cx: &App,
    ) -> Pixels {
        let count = self
            .detail
            .read(cx)
            .albums()
            .iter()
            .filter(|album| filter.matches(album.release_type))
            .count();
        let visible = match expanded {
            true => count,
            false => count.min(columns * RELEASE_ROWS),
        };
        let rows = visible.div_ceil(columns.max(1));
        if rows == 0 {
            return Pixels::ZERO;
        }

        let grid = CardGrid::layout(self.width);
        let card = Card::tile_height(grid.card, window, cx);
        card * rows as f32 + release_gap(window) * rows.saturating_sub(1) as f32
    }

    fn set_release_view(
        &mut self,
        filter: ReleaseFilter,
        expanded: bool,
        columns: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.release_height(
            self.release_filter,
            self.releases_expanded,
            columns,
            window,
            cx,
        );
        let new = self.release_height(filter, expanded, columns, window, cx);
        let offset = scrolled(self.scrollbar.read(cx).scroll());
        self.release_padding = match old > new {
            true => self.release_padding + (old - new).min(offset),
            false => (self.release_padding - (new - old)).max(Pixels::ZERO),
        };
        self.release_padding_offset = offset;
        self.release_filter = filter;
        self.releases_expanded = expanded;
        cx.notify();
    }

    fn settle_release_padding(&mut self, offset: Pixels) {
        if offset < self.release_padding_offset {
            self.release_padding =
                (self.release_padding - (self.release_padding_offset - offset)).max(Pixels::ZERO);
        }
        self.release_padding_offset = offset;
    }

    fn release_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let upward = event.delta.pixel_delta(window.line_height()).y;
        if upward <= Pixels::ZERO || self.release_padding <= Pixels::ZERO {
            return;
        }
        cx.notify();
    }

    /// How many releases the grid lists at a given packing: every one that passes the filter
    /// while it is expanded, and the first two rows of them while it is not.
    fn shown_releases(&self, columns: usize, cx: &App) -> usize {
        let count = self
            .detail
            .read(cx)
            .albums()
            .iter()
            .filter(|album| self.release_filter.matches(album.release_type))
            .count();

        match self.releases_expanded {
            true => count,
            false => count.min(columns * RELEASE_ROWS),
        }
    }

    /// Holds the releases grid still when the page changes width. A narrower page packs the
    /// cards into fewer columns and more rows, which slides whatever the user was looking at
    /// down the page; this moves the scroll by as much as the row under the top edge moved,
    /// so the same releases stay where they were. The row heights are uniform here, so the
    /// anchor is the first card of that row and nothing but the packing has to be remembered.
    fn hold_releases(&mut self, window: &Window, cx: &mut Context<Self>) {
        let grid = CardGrid::layout(self.width);
        let columns = grid.columns.max(1);
        let tile = Card::tile_height(grid.card, window, cx);
        let (was_columns, was_tile) = (self.release_columns, self.release_tile);
        self.release_columns = columns;
        self.release_tile = tile;
        if was_columns == 0 || (was_columns == columns && (was_tile - tile).abs() < px(0.5)) {
            return;
        }
        let Some(lead) = self.release_lead.get() else {
            return;
        };

        let scroll = self.scrollbar.read(cx).scroll().clone();
        let depth = scroll.bounds().origin.y - lead;
        if depth <= Pixels::ZERO {
            return;
        }
        let gap = release_gap(window);
        let before = self.shown_releases(was_columns, cx).div_ceil(was_columns);
        let (row, into) = Deck::at(&vec![was_tile; before], gap, depth);
        let share = match was_tile > Pixels::ZERO {
            true => into / was_tile,
            false => 0.,
        };

        let after = self.shown_releases(columns, cx).div_ceil(columns);
        let landed = (row * was_columns / columns).min(after.saturating_sub(1));
        let top = Deck::tops(&vec![tile; after], gap)
            .get(landed)
            .copied()
            .unwrap_or_default();
        let moved = top + tile * share - depth;
        if moved.abs() < px(0.5) {
            return;
        }

        let at = (-scroll.offset().y + moved).max(Pixels::ZERO);
        scroll.set_offset(point(Pixels::ZERO, -at));
    }

    /// The Releases grid. The rows go into a `Deck`, so an artist with a long discography
    /// builds only the rows on screen once the list is expanded, and the section still takes
    /// the height `release_height` reports, which is what the collapse padding is measured
    /// against.
    fn releases(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = *cx.theme();
        let detail = self.detail.read(cx);
        let albums = detail.albums();
        // A discography still on its way reads as loading only while nothing of it is up.
        let loading = detail.is_loading() || (albums.is_empty() && detail.is_filling());
        if albums.is_empty() && !loading {
            return None;
        }
        let local = detail.id().is_some_and(music::is_local_id);
        let filters = match loading {
            true => Vec::new(),
            false => cards::release_filters(local, albums.iter().map(|album| album.release_type)),
        };

        let grid = CardGrid::layout(self.width);
        let gap = release_gap(window);
        let releases = match loading {
            true => div()
                .flex()
                .flex_col()
                .gap(gap)
                .children((0..RELEASE_ROWS).map(|row| {
                    CardGrid::new(self.width).children((0..grid.columns).map(move |column| {
                        let index = row * grid.columns + column;
                        Card::skeleton(("artist-release-skeleton", index))
                            .tile(grid.card)
                            .into_any_element()
                    }))
                }))
                .into_any_element(),
            false => {
                let columns = grid.columns.max(1);
                let shown: Rc<[usize]> = albums
                    .iter()
                    .enumerate()
                    .filter(|(_, album)| self.release_filter.matches(album.release_type))
                    .map(|(index, _)| index)
                    .take(match self.releases_expanded {
                        true => usize::MAX,
                        false => columns * RELEASE_ROWS,
                    })
                    .collect();
                let rows = shown.len().div_ceil(columns);
                let height = Card::tile_height(grid.card, window, cx);
                let width = self.width;
                let card = grid.card;
                let opened = cx.entity().downgrade();
                let lead = self.release_lead.clone();

                Deck::new("artist-releases-deck")
                    .rows(std::iter::repeat_n(height, rows))
                    .gap(gap)
                    .on_measure(move |top, _, _| lead.set(Some(top)))
                    .draw(move |row, _, cx| {
                        let Some(view) = opened.upgrade() else {
                            return div().into_any_element();
                        };
                        let start = row * columns;
                        let end = (start + columns).min(shown.len());
                        // The cards are built straight off the shelf's albums. A row is
                        // rebuilt on every frame a scroll asks for, so cloning a release,
                        // its credits and all of its strings to get here would be the whole
                        // cost of scrolling a long discography.
                        let held = view.read(cx);
                        let listed = held.detail.read(cx).albums();
                        let cards = shown[start..end].iter().filter_map(|&index| {
                            let album = listed.get(index)?;

                            Some(
                                cards::album_card(
                                    ("artist-release", index),
                                    album,
                                    &held.playback,
                                    cx,
                                )
                                .tile(card)
                                .flat()
                                .into_any_element(),
                            )
                        });

                        CardGrid::new(width).children(cards).into_any_element()
                    })
                    .into_any_element()
            }
        };

        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .pt_6()
                .child(
                    div()
                        .text_size(theme.text(Text::Title))
                        .font_weight(FontWeight::BOLD)
                        .child(t!("artist-releases")),
                )
                .when(!filters.is_empty(), |this| {
                    this.child(
                        div()
                            .flex()
                            .gap_1()
                            .children(filters.into_iter().map(|filter| {
                                Button::new(filter.id())
                                    .label(filter.label())
                                    .small()
                                    .outline()
                                    .selected(self.release_filter == filter)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.release_filter == filter {
                                            return;
                                        }
                                        this.set_release_view(
                                            filter,
                                            false,
                                            grid.columns,
                                            window,
                                            cx,
                                        );
                                    }))
                            })),
                    )
                })
                .child(releases)
                .children(self.release_toggle(grid.columns, cx))
                .into_any_element(),
        )
    }

    fn release_toggle(&self, columns: usize, cx: &mut Context<Self>) -> Option<Button> {
        let count = self
            .detail
            .read(cx)
            .albums()
            .iter()
            .filter(|album| self.release_filter.matches(album.release_type))
            .count();
        if count <= columns * RELEASE_ROWS {
            return None;
        }
        let expanded = self.releases_expanded;

        Some(
            Button::new("artist-releases-more")
                .label(match expanded {
                    true => t!("artist-releases-less"),
                    false => t!("artist-releases-more"),
                })
                .trailing(chevron(expanded))
                .small()
                .ghost()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_release_view(this.release_filter, !expanded, columns, window, cx);
                })),
        )
    }

    /// The recommendation rail under the releases: what the artist guests on. A rail
    /// still on its way reads as skeletons only while nothing of it is up.
    fn recommended(
        &self,
        window: &Window,
        notify: &Rc<dyn Fn(&mut App)>,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let grid = CardGrid::layout(self.width);
        let card = grid.card;
        let columns = grid.columns.max(1);
        let detail = self.detail.read(cx);
        let filling = detail.is_filling();
        let appears = detail.appears_on().len();

        vec![match appears {
            0 if filling => Some(Rail::pending(
                t!("artist-appears-on"),
                card,
                columns,
                window,
                cx,
            )),
            0 => None,
            _ => {
                let opened = self.me.clone();
                Some(self.rails[0].render(
                    RailSpec {
                        tag: "artist-appears",
                        place: 0,
                        title: t!("artist-appears-on"),
                        count: appears,
                        tile: card,
                        columns,
                        tabs: None,
                    },
                    window,
                    cx,
                    notify,
                    move |index, _, cx| {
                        let Some(view) = opened.upgrade() else {
                            return div().into_any_element();
                        };
                        let held = view.read(cx);
                        let detail = held.detail.read(cx);
                        let Some(album) = detail.appears_on().get(index) else {
                            return div().into_any_element();
                        };
                        cards::album_card(("artist-appears", index), album, &held.playback, cx)
                            .tile(card)
                            .flat()
                            .into_any_element()
                    },
                ))
            }
        }]
        .into_iter()
        .flatten()
        .map(|rail| div().pt_6().child(rail).into_any_element())
        .collect()
    }

    fn listed(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let expanded = self.shown.get() > LISTED;
        let more = (self.popular.len() > LISTED).then(|| {
            Button::new("artist-popular-more")
                .label(match expanded {
                    true => t!("artist-popular-less"),
                    false => t!("artist-popular-more"),
                })
                .trailing(chevron(expanded))
                .small()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.shown.set(match expanded {
                        true => LISTED,
                        false => LISTED_MAX,
                    });
                    this.rebuild(cx);
                    cx.notify();
                }))
        });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                table(&self.table)
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border),
            )
            .children(more)
            .into_any_element()
    }

    fn playing_from(&self, cx: &App) -> Option<Origin> {
        let detail = self.detail.read(cx);
        let name = detail.artist()?.name.clone();

        Some(Origin::artist(detail.id()?).named(name))
    }

    fn popular(&self, cx: &mut Context<Self>) -> AnyElement {
        let tracks = self.popular.clone();
        let from = self.playing_from(cx);
        let pages = Shape::new(self.width, tracks.len()).pages;
        let queued = tracks.clone();
        let playback = self.playback.clone();
        let opened = cx.entity().downgrade();

        Picks::new(
            "artist-popular",
            tracks,
            self.playback.clone(),
            self.width,
            self.popular_page,
        )
        .title("artist-popular")
        .eyebrow(i18n::lookup("artist-popular-eyebrow", None))
        .vacancy("artist-popular-empty")
        .detailed()
        .loading(self.detail.read(cx).is_loading())
        .on_previous(cx.listener(|this, _, _, cx| {
            this.popular_page = this.popular_page.saturating_sub(1);
            this.track_context = None;
            cx.notify();
        }))
        .on_next(cx.listener(move |this, _, _, cx| {
            this.popular_page = (this.popular_page + 1).min(pages.saturating_sub(1));
            this.track_context = None;
            cx.notify();
        }))
        .on_context_menu(move |place, event, _, cx| {
            let Some(view) = opened.upgrade() else {
                return;
            };
            view.update(cx, |this, cx| {
                this.track_menu.reset(cx);
                this.track_context = Some((place, event.position));
                cx.notify();
            });
        })
        .on_start(move |place, cx| {
            playback.update(cx, |playback, cx| {
                playback.start(queued.as_ref().clone(), place, from.clone(), cx)
            });
        })
        .into_any_element()
    }

    fn about(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let artist = self.detail.read(cx).artist()?;
        let card = AboutArtist::new("artist-about", artist.name.clone())
            .cover(artist.cover_large.clone())
            .biography(artist.biography.clone())
            .on_open(cx.listener(|this, _, _, cx| {
                this.about_open = true;
                cx.notify();
            }));

        Some(div().pt_6().child(card).into_any_element())
    }

    /// Similarity comes from shared Archives reviewers, independently of provider suggestions.
    fn empath_similar(&self, cx: &mut Context<Self>) -> AnyElement {
        let empath = Sonora::global(cx).empath.clone();
        let state = empath.read(cx);
        if state.name.is_empty() || state.name != self.metal.read(cx).name {
            return div().into_any_element();
        }
        let theme = *cx.theme();
        let mut section = div()
            .flex()
            .flex_col()
            .gap_3()
            .mt_6()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(theme.text(Text::Title))
                            .font_weight(FontWeight::BOLD)
                            .child(t!("empath-similar-title")),
                    )
                    .child(
                        Button::new("empath-refresh-similar")
                            .small()
                            .outline()
                            .label(t!("metal-refresh"))
                            .disabled(state.similar_loading)
                            .on_click(move |_, _, cx| {
                                empath.update(cx, |state, cx| {
                                    let name = state.name.clone();
                                    let id = state
                                        .similar
                                        .as_ref()
                                        .map(|similar| similar.band.id.clone());
                                    state.load_similar(&name, id, true, cx);
                                });
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(t!("empath-similar-help")),
            );
        let empath = Sonora::global(cx).empath.clone();
        if state.similar_loading {
            section = section.child(Notice::new(
                t!("empath-loading-similar"),
                t!("empath-cache-help"),
            ));
        }
        if state.similar_failed {
            section = section
                .child(Notice::new(t!("empath-load-failed"), t!("empath-retry-help")).failed());
        }
        if state.similar_stale {
            section = section.child(crate::chrome::metal::notice(t!("empath-stale"), cx));
        }
        if let Some(similar) = &state.similar {
            section = section.child(crate::chrome::metal::empath_credit(
                &similar.updated,
                format!("https://www.furia.com/em/band{}.html", similar.band.id),
            ));
            let shown = match self.similar_expanded {
                true => similar.bands.len(),
                false => similar.bands.len().min(LISTED_MAX),
            };
            for (index, rank) in similar.bands.iter().take(shown).enumerate() {
                section = section.child(crate::chrome::metal::empath_row(
                    ("empath-similar-band", index),
                    rank,
                    index + 1,
                    true,
                    &empath,
                    cx,
                ));
            }
            if similar.bands.len() > LISTED_MAX {
                section = section.child(
                    Button::new("empath-expand-similar")
                        .small()
                        .ghost()
                        .label(match self.similar_expanded {
                            true => t!("empath-show-less"),
                            false => t!("empath-show-all", count = similar.bands.len()),
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.similar_expanded = !this.similar_expanded;
                            cx.notify();
                        })),
                );
            }
            if similar.bands.is_empty() {
                section = section.child(crate::chrome::metal::notice(t!("empath-no-similar"), cx));
            }
        } else if state.identities.len() > 1 {
            section = section.child(crate::chrome::metal::notice(
                t!("empath-choose-identity"),
                cx,
            ));
            for (index, band) in state.identities.iter().enumerate() {
                let picked = empath.clone();
                let id = band.id.clone();
                section = section.child(
                    Button::new(("empath-identity", index))
                        .small()
                        .outline()
                        .label(t!(
                            "empath-identity",
                            artist = band.name.clone(),
                            id = id.clone()
                        ))
                        .on_click(move |_, _, cx| {
                            picked.update(cx, |state, cx| {
                                let name = state.name.clone();
                                state.load_similar(&name, Some(id.clone()), false, cx);
                            })
                        }),
                );
            }
        } else if !state.similar_loading && !state.similar_failed {
            section = section.child(crate::chrome::metal::notice(t!("empath-no-similar"), cx));
        }
        section.into_any_element()
    }

    fn metal_discography(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::chrome::metal::{
            ReleaseContext, fetch_error, notice, release_cover, release_row,
        };
        let state = self.metal.read(cx);
        let theme = *cx.theme();
        let discovery = Sonora::global(cx).metal.clone();
        let source = state
            .band
            .as_ref()
            .map(|band| band.url.clone())
            .unwrap_or_else(|| metal::archives_search_url(&state.name));
        let mut section = div().pt_6().flex().flex_col().gap_2().child(
            div()
                .text_size(theme.text(Text::Title))
                .font_weight(FontWeight::BOLD)
                .child(t!("metal-discography")),
        );
        let refresh = state.loaded;
        let loaded = self.metal.clone();
        section = section.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("metal-discography-load")
                        .small()
                        .primary()
                        .label(match refresh {
                            true => t!("metal-refresh"),
                            false => t!("metal-load"),
                        })
                        .tooltip("metal-cache-policy")
                        .disabled(state.loading || state.dates_loading || state.name.is_empty())
                        .on_click(move |_, _, cx| {
                            loaded.update(cx, |state, cx| state.load(refresh, cx))
                        }),
                )
                .child(
                    Button::new("metal-open-archives")
                        .small()
                        .ghost()
                        .label(t!("metal-open-archives"))
                        .on_click(move |_, _, cx| cx.open_url(&source)),
                ),
        );
        section = section
            .when(state.loading, |section| {
                section.child(notice(t!("metal-loading"), cx))
            })
            .when(state.dates_loading, |section| {
                section.child(notice(t!("metal-checking-dates"), cx))
            })
            .when(discovery.read(cx).album_plays_loading, |section| {
                section.child(notice(t!("metal-loading-album-plays"), cx))
            })
            .when(discovery.read(cx).album_plays_failed, |section| {
                let minutes = discovery
                    .read(cx)
                    .album_plays_retry_at
                    .map(|at| {
                        at.saturating_sub(jiff::Timestamp::now().as_second().max(0) as u64)
                            .div_ceil(60)
                    })
                    .filter(|minutes| *minutes > 0);
                section.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(notice(
                            match minutes {
                                Some(minutes) => {
                                    t!("metal-album-plays-rate-limited", minutes = minutes)
                                }
                                None => t!("metal-album-plays-failed"),
                            },
                            cx,
                        ))
                        .child(
                            Button::new("metal-retry-album-plays")
                                .small()
                                .outline()
                                .label(t!("metal-retry-album-plays"))
                                .on_click(|_, _, cx| {
                                    Sonora::global(cx)
                                        .metal
                                        .clone()
                                        .update(cx, |state, cx| state.load_album_history(cx))
                                }),
                        ),
                )
            })
            .when_some(state.dates_error.as_ref(), |section, error| {
                section.child(notice(fetch_error(error), cx))
            })
            .when(state.expired, |section| {
                section.child(notice(t!("metal-cache-expired"), cx))
            })
            .when_some(state.error.as_ref(), |section, error| {
                section.child(notice(fetch_error(error), cx))
            })
            .when_some(
                discovery.read(cx).reader_error.as_ref(),
                |section, error| section.child(notice(fetch_error(error), cx)),
            );
        if !state.loaded {
            return section
                .child(notice(t!("metal-load-help"), cx))
                .into_any_element();
        }
        if let Some(band) = &state.band {
            let changed = self.metal.clone();
            let provider_id = self.detail.read(cx).id().map(str::to_owned);
            section = section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(format!("{} / {} / {}", band.name, band.genre, band.country))
                    .child(
                        Button::new("metal-change-band")
                            .small()
                            .ghost()
                            .label(t!("metal-change-band"))
                            .on_click(move |_, _, cx| {
                                if let Some(provider_id) = provider_id.as_ref() {
                                    Sonora::global(cx).empath.clone().update(cx, |state, cx| {
                                        state.forget_archives(provider_id, cx)
                                    });
                                }
                                changed.update(cx, |state, cx| state.change_band(cx))
                            }),
                    ),
            );
            section = section.child(
                TabBar::new("metal-discography-tabs")
                    .max_w(self.width)
                    .items(DiscographyTab::TABS.into_iter().map(|tab| {
                        Button::new(tab.key())
                            .small()
                            .outline()
                            .selected(self.metal_tab == tab)
                            .label(i18n::lookup(tab.key(), None))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.metal_tab = tab;
                                cx.notify();
                            }))
                    })),
            );
            let albums = self.detail.read(cx).albums();
            let mut shown = 0;
            for group in DiscographyTab::GROUPS {
                if self.metal_tab != DiscographyTab::All && self.metal_tab != group {
                    continue;
                }
                let releases = state
                    .releases
                    .iter()
                    .enumerate()
                    .filter(|(_, release)| group.includes(release))
                    .collect::<Vec<_>>();
                if releases.is_empty() {
                    continue;
                }
                shown += releases.len();
                section = section.child(
                    div()
                        .pt_4()
                        .pb_2()
                        .font_weight(FontWeight::BOLD)
                        .text_size(theme.text(Text::Title))
                        .child(i18n::lookup(group.key(), None)),
                );
                for (index, release) in releases {
                    section = section.child(release_row(
                        ("metal-discography-release", index),
                        release,
                        release_cover(release, albums),
                        ReleaseContext::Artist(discovery.read(cx).album_plays(release)),
                        &discovery,
                        cx,
                    ));
                }
            }
            if shown == 0 && !state.releases.is_empty() {
                section = section.child(notice(t!("metal-no-tab-releases"), cx));
            }
            if state.releases.is_empty() && !state.loading && state.error.is_none() {
                section = section.child(notice(t!("metal-no-discography"), cx));
            }
        } else if !state.candidates.is_empty() {
            section = section.child(notice(t!("metal-select-band"), cx));
            for (index, band) in state.candidates.iter().enumerate() {
                let band = band.clone();
                let selected = self.metal.clone();
                section = section.child(
                    Button::new(("metal-band-candidate", index))
                        .outline()
                        .label(format!("{} / {} / {}", band.name, band.genre, band.country))
                        .on_click(move |_, _, cx| {
                            selected.update(cx, |state, cx| state.choose(band.clone(), cx))
                        }),
                );
            }
        } else if !state.loading && state.error.is_none() {
            section = section.child(notice(t!("metal-no-band"), cx));
        }
        section.into_any_element()
    }

    /// Closes the About the artist dialog when it is open, and says whether it was.
    pub(crate) fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.about_open {
            return false;
        }
        self.about_open = false;
        cx.notify();
        true
    }

    fn about_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.about_open {
            return None;
        }
        let artist = self.detail.read(cx).artist()?;

        Some(
            about_modal(
                artist.name.clone().into(),
                artist.biography.clone(),
                None,
                &self.about_bar,
                cx,
            )
            .action(
                Button::new("artist-about-close")
                    .label(t!("common-dismiss"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.about_open = false;
                        cx.notify();
                    })),
            )
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.about_open = false;
                cx.notify();
            }))
            .into_any_element(),
        )
    }

    /// The page an artist shows instead of its hero and its tables when it cannot be read: the
    /// No connection state as soon as the network is gone, whatever this page happens to hold,
    /// and the failure of its own load otherwise. Opening the same artist again is the retry.
    fn failure(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let id = self.detail.read(cx).id()?.to_owned();
        let reason = match trouble::unreachable(&id, cx) {
            true => None,
            false => Some(self.detail.read(cx).error()?.to_owned()),
        };
        let detail = self.detail.clone();

        Some(
            trouble::lost(
                "artist-lost",
                t!("trouble-not-loaded"),
                reason.as_deref(),
                move |_, _, cx| {
                    let id = id.clone();
                    detail.update(cx, |detail, cx| detail.open(&id, cx));
                },
                cx,
            )
            .size_full()
            .into_any_element(),
        )
    }
}

impl Tooled for ArtistView {
    fn toolbar(&self) -> Entity<Toolbar> {
        self.toolbar.clone()
    }

    fn tools(&self, _cx: &App) -> Vec<AnyElement> {
        let viewed = self.me.clone();

        vec![tools::views(&self.popovers, self.mode, move |mode, cx| {
            viewed.update(cx, |view, cx| view.set_mode(mode, cx)).ok();
        })]
    }
}

impl Render for ArtistView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(failure) = self.failure(cx) {
            return div().relative().size_full().child(failure);
        }

        let theme = *cx.theme();
        let inset = theme.metrics.inset;
        let previous = self.width;
        page::resize(&self.table, &mut self.width, inset, window, cx);

        let scroll = self.scrollbar.read(cx).scroll().clone();
        self.settle_release_padding(scrolled(&scroll));
        self.hold_releases(window, cx);
        if self.width != previous {
            self.scrollbar
                .update(cx, |bar, cx| bar.set_max_offset(None, cx));
        }
        let columns = Shape::new(self.width, self.popular.len()).columns;
        if self.popular_columns != columns {
            self.popular_columns = columns;
            self.popular_page = 0;
        }
        if self.mode == Mode::List {
            self.table.claim(cx);
            let viewport = page::viewport(&scroll, inset, window);
            self.table
                .update(cx, |table, _| table.set_viewport(viewport));
        }

        let picked = self.track_context.and_then(|(place, position)| {
            self.popular
                .get(place)
                .cloned()
                .map(|track| (track, position))
        });
        let track_menu = picked.map(|(track, position)| {
            Popup::new(position, self.track_menu.for_track(&track, cx)).on_close(cx.listener(
                |this, _, _, cx| {
                    this.track_context = None;
                    cx.notify();
                },
            ))
        });

        let listed = self.mode == Mode::List;
        let release_padding = self.release_padding;
        let head = self.header(cx);
        let tracks = match self.mode {
            Mode::Grid => self.popular(cx),
            Mode::List => self.listed(cx),
        };
        let grid = self.releases(window, cx);
        let about = self.about(cx);
        while self.rails.is_empty() {
            self.rails.push(Rail::new(cx.entity_id()));
        }
        for rail in &self.rails {
            rail.sync();
        }
        let weak = cx.entity().downgrade();
        let notify: Rc<dyn Fn(&mut App)> = Rc::new(move |cx: &mut App| {
            weak.update(cx, |_, cx| cx.notify()).ok();
        });
        let rails = self.recommended(window, &notify, cx);
        let page = Scroller::new("artist-page", &self.scrollbar)
            .bottom_inset(crate::chrome::Chrome::bottom(cx))
            .px(inset)
            .pt(inset)
            .pb(inset)
            .on_scroll_wheel(cx.listener(Self::release_scroll))
            .child(div().child(head).when(listed, |this| {
                this.child(
                    div()
                        .pb_3()
                        .text_size(theme.text(Text::Title))
                        .font_weight(FontWeight::BOLD)
                        .child(t!("artist-popular")),
                )
            }))
            .child(tracks)
            .child(self.metal_discography(cx))
            .children(grid)
            .child(self.empath_similar(cx))
            .children(rails)
            .children(about)
            .when(release_padding > Pixels::ZERO, |this| {
                this.child(div().h(release_padding).flex_none())
            });

        div()
            .relative()
            .size_full()
            .child(page)
            .when_some(track_menu, |this, menu| this.child(menu))
            .children(self.about_dialog(cx))
            .children(crate::chrome::metal::empath_modal(
                &Sonora::global(cx).empath,
                cx,
            ))
            .children(crate::chrome::metal::album_modal(
                &Sonora::global(cx).metal,
                cx,
            ))
    }
}

fn release_gap(window: &Window) -> Pixels {
    snapped(window.rem_size() * 1.5, window)
}

fn chevron(expanded: bool) -> &'static str {
    match expanded {
        true => "icons/chevron-up.svg",
        false => "icons/chevron-down.svg",
    }
}
