use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, Context, Entity, FontWeight, Render, ScrollHandle, Window, div};
use i18n::t;
use metal::normalize;
use state::{CatalogLookup, CatalogOpened, Empath, MetalDiscovery, Sonora};
use ui::{
    ActiveTheme as _, Button, Deck, Input, MenuItem, Notice, Picker, Popovers, Scrollbar, Scroller,
    TabBar, Text,
};

use crate::chrome::Chrome;

use crate::chrome::metal::{
    ReleaseContext, album_modal, empath_credit, empath_modal, empath_row, fetch_error, notice,
    release_row,
};

mod listening;
use listening::{ListeningKind, ListeningRow};

/// Each Discover section loads only its own data; listening charts read existing caches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Releases,
    Genres,
    Listening,
}

#[derive(Clone, Copy, PartialEq, Eq)]
/// Filters are local and never trigger another Archives request.
enum Filter {
    All,
    Albums,
    Familiar,
    NewToYou,
}

#[derive(Clone)]
/// Timeline headings and indices keep the large release listing virtualized.
enum Row {
    Day(String),
    Release(usize),
}

/// A chronological release timeline with local album, history, and recommendation filters.
pub(crate) struct DiscoverView {
    discovery: Entity<MetalDiscovery>,
    empath: Entity<Empath>,
    catalog_lookup: Entity<CatalogLookup>,
    section: Section,
    listening_kind: ListeningKind,
    listening_rows: Rc<Vec<ListeningRow>>,
    style: usize,
    genre_rows: Rc<Vec<usize>>,
    popovers: Popovers,
    input: Entity<Input>,
    filter: Filter,
    rows: Rc<Vec<Row>>,
    scrollbar: Entity<Scrollbar>,
}

impl DiscoverView {
    pub(crate) fn new(discovery: Entity<MetalDiscovery>, cx: &mut Context<Self>) -> Self {
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            Input::new("metal-filter-hint", cx)
                .icon("icons/search.svg")
                .clearable()
        });
        cx.observe(&discovery, |this, _, cx| {
            this.rebuild(cx);
            this.rebuild_listening(cx);
            cx.notify();
        })
        .detach();
        let empath = Sonora::global(cx).empath.clone();
        let session = Sonora::global(cx).session.clone();
        let catalog_lookup = cx.new(|cx| CatalogLookup::new(session, cx));
        cx.observe(&catalog_lookup, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&catalog_lookup, |_, _, event, cx| {
            let destination = match event {
                CatalogOpened::Artist(id) => router::Destination::Artist(id.clone().into()),
                CatalogOpened::Album(id) => router::Destination::Album(id.clone().into()),
            };
            router::navigate(destination, cx);
        })
        .detach();
        cx.observe(&empath, |this, _, cx| {
            this.rebuild_genres(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&input, |this, _, cx| {
            this.rebuild(cx);
            this.rebuild_genres(cx);
            this.rebuild_listening(cx);
            this.rewind(cx);
            cx.notify();
        })
        .detach();
        let me = cx.entity_id();
        Self {
            discovery,
            empath,
            catalog_lookup,
            section: Section::Releases,
            listening_kind: ListeningKind::Artists,
            listening_rows: Rc::new(Vec::new()),
            style: 0,
            genre_rows: Rc::new(Vec::new()),
            popovers: Popovers::default(),
            input,
            filter: Filter::Albums,
            rows: Rc::new(Vec::new()),
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
        }
    }

    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        match self.section {
            Section::Releases => self.discovery.update(cx, |state, cx| state.load(cx)),
            Section::Genres => self
                .empath
                .update(cx, |state, cx| state.load_genres(false, cx)),
            Section::Listening => self
                .discovery
                .update(cx, |state, cx| state.restore_listening(cx)),
        }
        self.rebuild(cx);
        self.rebuild_genres(cx);
        self.rebuild_listening(cx);
    }

    /// Genres are filtered locally, without fetching one page per style or band.
    fn rebuild_genres(&mut self, cx: &Context<Self>) {
        let query = normalize(self.input.read(cx).text());
        self.genre_rows = Rc::new(
            self.empath
                .read(cx)
                .genres
                .as_ref()
                .and_then(|genres| genres.styles.get(self.style))
                .map(|style| {
                    style
                        .bands
                        .iter()
                        .enumerate()
                        .filter_map(|(index, rank)| {
                            normalize(&rank.band.name).contains(&query).then_some(index)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
    }

    fn page_tabs(&self, cx: &Context<Self>) -> AnyElement {
        TabBar::new("discover-sections")
            .items(
                [
                    (Section::Releases, "empath-new-releases"),
                    (Section::Genres, "empath-top-genres"),
                    (Section::Listening, "listening-tab"),
                ]
                .into_iter()
                .map(|(section, key)| {
                    Button::new(key)
                        .small()
                        .label(i18n::lookup(key, None))
                        .selected(self.section == section)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.catalog_lookup
                                .update(cx, |state, cx| state.dismiss(cx));
                            this.section = section;
                            this.popovers.close();
                            this.rewind(cx);
                            this.load(cx);
                            cx.notify();
                        }))
                }),
            )
            .into_any_element()
    }

    fn genre_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let genres = self.empath.read(cx).genres.as_ref();
        let rank = genres
            .and_then(|genres| genres.styles.get(self.style))
            .and_then(|style| {
                self.genre_rows
                    .get(index)
                    .and_then(|index| style.bands.get(*index))
            });
        match rank {
            Some(rank) => empath_row(
                ("empath-genre-band", index),
                rank,
                self.genre_rows[index] + 1,
                false,
                &self.empath,
                cx,
            ),
            None => div().into_any_element(),
        }
    }

    fn genres_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let state = self.empath.read(cx);
        let style = state
            .genres
            .as_ref()
            .and_then(|genres| genres.styles.get(self.style));
        let mut header = div()
            .flex()
            .flex_col()
            .gap_4()
            .pb_6()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(theme.text(Text::Display))
                                    .font_weight(FontWeight::BOLD)
                                    .child(t!("nav-discover")),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("empath-genres-description")),
                            ),
                    )
                    .child(
                        Button::new("empath-refresh-genres")
                            .small()
                            .outline()
                            .icon("icons/refresh-cw.svg")
                            .label(t!("metal-refresh"))
                            .disabled(state.genres_loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.empath
                                    .update(cx, |state, cx| state.load_genres(true, cx));
                            })),
                    ),
            )
            .child(self.page_tabs(cx));
        if let Some(style) = style {
            let styles = &state.genres.as_ref().expect("style has a source").styles;
            header = header.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        Picker::new("empath-style", &self.popovers, style.name.clone())
                            .left()
                            .width(Picker::REGULAR)
                            .items(styles.iter().enumerate().map(|(index, style)| {
                                MenuItem::new(("empath-style-choice", index), style.name.clone())
                                    .checked(self.style == index)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.style = index;
                                        this.rebuild_genres(cx);
                                        this.rewind(cx);
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(empath_credit(
                        &state.genres.as_ref().expect("style has a source").updated,
                        "https://www.furia.com/em/bands-style.html".into(),
                    )),
            );
        }
        header = header
            .child(self.input.clone())
            .child(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(t!("empath-ranking-help")),
            )
            .when(style.is_some(), |header| {
                header.child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(t!("empath-band-count", count = self.genre_rows.len())),
                )
            });
        let rows = self.genre_rows.len();
        Scroller::new("empath-genres-page", &self.scrollbar)
            .px(theme.metrics.inset)
            .py(theme.metrics.inset)
            .max_w(Chrome::content(window, cx))
            .child(header)
            .when(state.genres_loading, |page| {
                page.child(Notice::new(t!("empath-loading-genres"), t!("empath-cache-help")).mb_4())
            })
            .when(state.genres_failed, |page| {
                page.child(
                    Notice::new(t!("empath-load-failed"), t!("empath-retry-help"))
                        .failed()
                        .mb_4(),
                )
            })
            .when(state.genres_stale, |page| {
                page.child(notice(t!("empath-stale"), cx))
            })
            .when(
                style.is_some() && !state.genres_loading && rows == 0,
                |page| page.child(notice(t!("empath-no-filter-match"), cx)),
            )
            .child(
                Deck::new("empath-genre-bands")
                    .rows((0..rows).map(|_| theme.metrics.row * 2. + theme.metrics.pad * 2.))
                    .draw(cx.processor(|this, index, _, cx| this.genre_row(index, cx))),
            )
            .into_any_element()
    }

    fn rewind(&mut self, cx: &mut Context<Self>) {
        self.scrollbar
            .update(cx, |bar, _| bar.scroll().set_offset(gpui::Point::default()));
    }

    fn rebuild(&mut self, cx: &Context<Self>) {
        let state = self.discovery.read(cx);
        if state.history.artists.is_empty()
            && matches!(self.filter, Filter::Familiar | Filter::NewToYou)
        {
            self.filter = Filter::Albums;
        }
        let query = normalize(self.input.read(cx).text());
        let mut indices: Vec<_> = state
            .releases
            .iter()
            .enumerate()
            .filter_map(|(index, release)| {
                let allowed = match self.filter {
                    Filter::All => true,
                    Filter::Albums => release.kind == "Full-length",
                    Filter::Familiar => state.playcount(&release.band.name) > 0,
                    Filter::NewToYou => {
                        release.kind == "Full-length"
                            && state.playcount(&release.band.name) <= 5
                            && state
                                .info
                                .get(&normalize(&release.band.name))
                                .is_some_and(|info| {
                                    info.listeners.is_some_and(|listeners| listeners >= 25_000)
                                })
                    }
                };
                (allowed
                    && normalize(&format!(
                        "{} {} {} {}",
                        release.title, release.band.name, release.band.genre, release.kind
                    ))
                    .contains(&query))
                .then_some(index)
            })
            .collect();
        indices.sort_by(|a, b| {
            let a = &state.releases[*a];
            let b = &state.releases[*b];
            b.date
                .cmp(&a.date)
                .then_with(|| {
                    state
                        .playcount(&b.band.name)
                        .cmp(&state.playcount(&a.band.name))
                })
                .then_with(|| a.band.name.cmp(&b.band.name))
        });
        let mut rows = Vec::new();
        let mut date = String::new();
        for index in indices {
            let release = &state.releases[index];
            if date != release.date {
                date = release.date.clone();
                rows.push(Row::Day(date.clone()));
            }
            rows.push(Row::Release(index));
        }
        self.rows = Rc::new(rows);
    }

    fn row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        match self.rows.get(index) {
            Some(Row::Release(release)) => release_row(
                ("metal-release", index),
                &self.discovery.read(cx).releases[*release],
                None,
                ReleaseContext::Discover,
                &self.discovery,
                cx,
            ),
            Some(Row::Day(date)) => div()
                .h_full()
                .flex()
                .items_center()
                .font_weight(FontWeight::BOLD)
                .text_size(theme.text(Text::Title))
                .child(
                    date.parse::<jiff::civil::Date>()
                        .map(|date| date.strftime("%A, %B %-d").to_string())
                        .unwrap_or_else(|_| date.clone()),
                )
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

impl Render for DiscoverView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.section == Section::Listening {
            return div()
                .relative()
                .size_full()
                .child(self.listening_page(window, cx))
                .children(self.listening_lookup_modal(cx));
        }
        if self.section == Section::Genres {
            let page = self.genres_page(window, cx);
            return div()
                .relative()
                .size_full()
                .child(page)
                .children(empath_modal(&self.empath, cx));
        }
        let theme = *cx.theme();
        let width = Chrome::content(window, cx);
        let state = self.discovery.read(cx);
        let loading = state.loading;
        let verifying = state.verifying();
        let loaded = state.has_results();
        let error = state.error.clone();
        let reader_error = state.reader_error.clone();
        let expired = state.expired;
        let personal = !state.history.artists.is_empty();
        let enriching = state.enriching;
        let (checked, candidates) = state.recommendation_progress();
        let account = state.lastfm_user().map(str::to_owned);
        let history_failed = state.history_error.is_some();
        let enrichment_failed = state.enrichment_error.is_some();
        let range = format!(
            "{} – {}",
            state.from.strftime("%b %-d, %Y"),
            state.to.strftime("%b %-d, %Y")
        );
        let visible = self
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Release(_)))
            .count();
        let total = state.release_total;
        let count = state.releases.len();
        let tabs = [
            (Filter::Albums, "metal-full-lengths"),
            (Filter::All, "metal-all"),
            (Filter::Familiar, "metal-known"),
            (Filter::NewToYou, "metal-new-to-you"),
        ];
        let header = div()
            .flex()
            .flex_col()
            .gap_4()
            .pb_6()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(theme.text(Text::Display))
                                    .font_weight(FontWeight::BOLD)
                                    .child(t!("nav-discover")),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("metal-discover-description")),
                            ),
                    )
                    .child(
                        Button::new("metal-refresh")
                            .small()
                            .outline()
                            .icon("icons/refresh-cw.svg")
                            .label(t!("metal-refresh"))
                            .tooltip("metal-discover-cache-policy")
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discovery.update(cx, |state, cx| state.fetch(true, cx));
                            })),
                    ),
            )
            .child(self.page_tabs(cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("metal-previous-week")
                            .small()
                            .outline()
                            .icon("icons/chevron-left.svg")
                            .tooltip("common-previous")
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discovery.update(cx, |state, cx| state.shift(-7, cx));
                                this.rewind(cx);
                            })),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(theme.text(Text::Large))
                            .child(range),
                    )
                    .child(
                        Button::new("metal-next-week")
                            .small()
                            .outline()
                            .icon("icons/chevron-right.svg")
                            .tooltip("common-next")
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discovery.update(cx, |state, cx| state.shift(7, cx));
                                this.rewind(cx);
                            })),
                    )
                    .child(
                        Button::new("metal-today")
                            .small()
                            .ghost()
                            .label(t!("metal-this-week"))
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discovery.update(cx, |state, cx| state.today(cx));
                                this.rewind(cx);
                            })),
                    ),
            )
            .child(
                TabBar::new("metal-discover-filters").max_w(width).items(
                    tabs.into_iter()
                        .filter(|(filter, _)| {
                            personal || matches!(filter, Filter::Albums | Filter::All)
                        })
                        .map(|(filter, key)| {
                            Button::new(key)
                                .small()
                                .selected(self.filter == filter)
                                .label(i18n::lookup(key, None))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.filter = filter;
                                    this.rebuild(cx);
                                    this.rewind(cx);
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(self.input.clone())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .when(loaded, |this| {
                        this.child(
                            div()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(t!("metal-visible-count", count = visible)),
                        )
                    })
                    .when_some(account.filter(|_| personal), |this, account| {
                        this.child(
                            div()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(t!("metal-personalized", user = account)),
                        )
                    }),
            )
            .when(self.filter == Filter::NewToYou, |this| {
                this.child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(t!("metal-new-to-you-help")),
                )
            })
            .when(self.filter == Filter::Familiar, |this| {
                this.child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(t!("metal-familiar-help")),
                )
            });
        let rows = self.rows.clone();
        let page = Scroller::new("discover-page", &self.scrollbar)
            .px(theme.metrics.inset)
            .py(theme.metrics.inset)
            .child(header)
            .when(loading, |this| {
                this.child(
                    Notice::new(
                        t!("metal-discover-loading"),
                        match verifying {
                            true => t!("metal-discover-verifying"),
                            false => t!("metal-discover-cache-policy"),
                        },
                    )
                    .icon("icons/disc-3.svg")
                    .mb_4(),
                )
            })
            .when_some(error.as_ref(), |this, error| {
                this.child(
                    Notice::new(t!("metal-discover-failed"), fetch_error(error))
                        .failed()
                        .mb_4(),
                )
            })
            .when_some(reader_error.as_ref(), |this, error| {
                this.child(notice(fetch_error(error), cx))
            })
            .when(expired && error.is_some(), |this| {
                this.child(notice(t!("metal-discover-stale"), cx))
            })
            .when(history_failed || enrichment_failed, |this| {
                this.child(notice(t!("metal-personalization-failed"), cx))
            })
            .when(total > count, |this| {
                this.child(notice(
                    t!("metal-partial", count = count, total = total),
                    cx,
                ))
            })
            .when(self.filter == Filter::NewToYou, |this| {
                this.child(notice(
                    t!(
                        "metal-personalization-progress",
                        checked = checked,
                        total = candidates
                    ),
                    cx,
                ))
                .when(enriching, |this| {
                    this.child(notice(t!("metal-personalizing"), cx))
                })
            })
            .when(
                loaded
                    && !loading
                    && !enriching
                    && error.is_none()
                    && self.rows.is_empty()
                    && (self.filter != Filter::NewToYou
                        || (checked == candidates && !history_failed)),
                |this| {
                    this.child(
                        Notice::new(t!("metal-no-releases"), t!("metal-discover-empty-help"))
                            .icon("icons/search.svg"),
                    )
                },
            )
            .child(
                Deck::new("metal-discover-timeline")
                    .rows(rows.iter().map(|row| match row {
                        Row::Release(_) => theme.metrics.row * 2. + theme.metrics.pad * 2.,
                        _ => theme.metrics.header,
                    }))
                    .draw(cx.processor(|this, index, _, cx| this.row(index, cx))),
            );
        div()
            .relative()
            .size_full()
            .child(page)
            .children(album_modal(&self.discovery, cx))
    }
}
