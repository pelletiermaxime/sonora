use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Entity, FontWeight, PathPromptOptions, Render, ScrollHandle, Task, Window,
    div,
};
use i18n::t;
use metal::normalize;
use state::MetalDiscovery;
use ui::{ActiveTheme as _, Button, Deck, Input, Scrollbar, Scroller, Text};

use crate::chrome::metal::{album_modal, notice, release_row};

#[derive(Clone, Copy, PartialEq, Eq)]
/// Filters are local and never trigger another Archives request.
enum Filter {
    All,
    Albums,
    Familiar,
}

#[derive(Clone)]
/// Timeline headings and indices keep the large release listing virtualized.
enum Row {
    Section(&'static str),
    Day(String),
    Release(usize),
}

/// A locally filtered release timeline, with familiar bands before chronological browsing.
pub(crate) struct DiscoverView {
    discovery: Entity<MetalDiscovery>,
    input: Entity<Input>,
    filter: Filter,
    rows: Rc<Vec<Row>>,
    scrollbar: Entity<Scrollbar>,
    picker: Option<Task<()>>,
}

impl DiscoverView {
    pub(crate) fn new(discovery: Entity<MetalDiscovery>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            Input::new("metal-filter-hint", cx)
                .icon("icons/search.svg")
                .clearable()
        });
        cx.observe(&discovery, |this, _, cx| {
            this.rebuild(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&input, |this, _, cx| {
            this.rebuild(cx);
            this.rewind(cx);
            cx.notify();
        })
        .detach();
        let me = cx.entity_id();
        Self {
            discovery,
            input,
            filter: Filter::All,
            rows: Rc::new(Vec::new()),
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            picker: None,
        }
    }

    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        self.discovery.update(cx, |state, cx| state.load(cx));
        self.rebuild(cx);
    }

    fn rewind(&mut self, cx: &mut Context<Self>) {
        self.scrollbar
            .update(cx, |bar, _| bar.scroll().set_offset(gpui::Point::default()));
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self.picker = Some(cx.spawn(async move |this, cx| {
            let selected = chosen.await;
            this.update(cx, |this, cx| {
                this.picker = None;
                if let Ok(Ok(Some(paths))) = selected
                    && let Some(path) = paths.into_iter().next()
                {
                    this.discovery
                        .update(cx, |state, cx| state.import(path, cx));
                    this.rewind(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn rebuild(&mut self, cx: &Context<Self>) {
        let state = self.discovery.read(cx);
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
        let mut familiar: Vec<_> = indices
            .iter()
            .copied()
            .filter(|index| state.playcount(&state.releases[*index].band.name) > 0)
            .collect();
        familiar.sort_by_key(|index| {
            std::cmp::Reverse(state.playcount(&state.releases[*index].band.name))
        });
        let mut rows = Vec::new();
        if !familiar.is_empty() {
            rows.push(Row::Section("metal-familiar"));
            rows.extend(familiar.into_iter().take(12).map(Row::Release));
        }
        let discoveries: Vec<_> = indices
            .iter()
            .copied()
            .filter(|index| {
                let release = &state.releases[*index];
                release.kind == "Full-length"
                    && state.playcount(&release.band.name) <= 5
                    && state
                        .info
                        .get(&normalize(&release.band.name))
                        .is_some_and(|info| {
                            info.listeners.is_some_and(|listeners| listeners >= 25_000)
                                && info.tags.iter().any(|tag| normalize(tag).contains("metal"))
                        })
            })
            .collect();
        if !discoveries.is_empty() {
            rows.push(Row::Section("metal-worth-checking"));
            rows.extend(discoveries.into_iter().take(12).map(Row::Release));
        }
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
                &self.discovery,
                cx,
            ),
            Some(Row::Day(date)) => div()
                .h_full()
                .flex()
                .items_center()
                .font_weight(FontWeight::BOLD)
                .text_size(theme.text(Text::Title))
                .child(date.clone())
                .into_any_element(),
            Some(Row::Section(key)) => div()
                .h_full()
                .flex()
                .items_center()
                .font_weight(FontWeight::BOLD)
                .text_size(theme.text(Text::Title))
                .text_color(theme.primary)
                .child(i18n::lookup(key, None))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

impl Render for DiscoverView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let state = self.discovery.read(cx);
        let loading = state.loading;
        let error = state.error.clone();
        let failed = error.is_some();
        let history_error = state.history_error.clone();
        let enrichment_error = state.enrichment_error.clone();
        let enriching = state.enriching;
        let count = state.releases.len();
        let artists = state.history.artists.len();
        let range = format!("{} - {}", state.from, state.to);
        let source = state.history.source.clone();
        let imported = state.imported;
        let total = state.release_total;
        let mut filters = div().flex().flex_wrap().gap_2();
        for (index, (filter, key)) in [
            (Filter::All, "metal-all"),
            (Filter::Albums, "metal-full-lengths"),
            (Filter::Familiar, "metal-known"),
        ]
        .into_iter()
        .enumerate()
        {
            filters = filters.child(
                Button::new(("metal-filter", index))
                    .small()
                    .outline()
                    .selected(self.filter == filter)
                    .label(i18n::lookup(key, None))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        this.rebuild(cx);
                        this.rewind(cx);
                        cx.notify();
                    })),
            );
        }
        let rows = self.rows.clone();
        let page = Scroller::new("discover-page", &self.scrollbar)
            .px(theme.metrics.inset)
            .py(theme.metrics.inset)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pb_4()
                    .child(
                        div()
                            .text_size(theme.text(Text::Display))
                            .font_weight(FontWeight::BOLD)
                            .child(t!("nav-discover")),
                    )
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(t!("metal-discover-description")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("metal-previous-week")
                                    .small()
                                    .icon("icons/chevron-left.svg")
                                    .label(t!("common-previous"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discovery.update(cx, |state, cx| state.shift(-7, cx));
                                        this.rewind(cx);
                                    })),
                            )
                            .child(range)
                            .child(
                                Button::new("metal-next-week")
                                    .small()
                                    .icon("icons/chevron-right.svg")
                                    .label(t!("common-next"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discovery.update(cx, |state, cx| state.shift(7, cx));
                                        this.rewind(cx);
                                    })),
                            )
                            .child(
                                Button::new("metal-today")
                                    .small()
                                    .label(t!("metal-this-week"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discovery.update(cx, |state, cx| state.today(cx));
                                        this.rewind(cx);
                                    })),
                            )
                            .child(
                                Button::new("metal-import")
                                    .small()
                                    .outline()
                                    .label(t!("metal-import"))
                                    .tooltip("metal-import-help")
                                    .disabled(loading)
                                    .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(t!("metal-summary", count = count, artists = artists))
                            .when(!source.is_empty(), |this| {
                                this.child(t!("metal-history-source", source = source))
                            }),
                    )
                    .when(imported && total <= count, |this| {
                        this.child(
                            div()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(t!("metal-imported")),
                        )
                    })
                    .when(total > count, |this| {
                        this.child(
                            div()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(t!("metal-partial", count = count, total = total)),
                        )
                    })
                    .child(self.input.clone())
                    .child(filters)
                    .child(
                        Button::new("metal-enrich")
                            .small()
                            .outline()
                            .tooltip("metal-request-policy")
                            .label(match enriching {
                                true => t!("metal-enriching"),
                                false => t!("metal-enrich"),
                            })
                            .disabled(loading || enriching || artists == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discovery.update(cx, |state, cx| state.enrich(cx))
                            })),
                    ),
            )
            .when(loading, |this| this.child(notice(t!("metal-loading"), cx)))
            .when_some(error, |this, error| {
                this.child(notice(error, cx)).child(
                    Button::new("metal-retry")
                        .label(t!("metal-retry"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.discovery.update(cx, |state, cx| state.retry(cx))
                        })),
                )
            })
            .when_some(history_error, |this, error| this.child(notice(error, cx)))
            .when_some(enrichment_error, |this, error| {
                this.child(notice(error, cx))
            })
            .when(!loading && !failed && self.rows.is_empty(), |this| {
                this.child(notice(t!("metal-no-releases"), cx))
            })
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
