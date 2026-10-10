use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, FontWeight, Window, div};
use i18n::t;
use metal::normalize;
use state::{CatalogCandidate, CatalogQuery};
use ui::{
    ActiveTheme as _, Button, Card, Deck, InlineLink, InlineLinks, Modal, Notice, Scroller, TabBar,
    Text,
};

use super::{DiscoverView, Section};
use crate::chrome::Chrome;

/// Selects an all-time chart from the existing shared Last.fm history.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ListeningKind {
    Artists,
    Albums,
}

/// Ranked source names and counts; filtering preserves the original all-time rank.
pub(super) struct ListeningRow {
    artist: String,
    album: Option<String>,
    plays: u64,
    rank: usize,
}

impl DiscoverView {
    /// Sorts only the saved charts; unknown or zero album counts never become chart entries.
    pub(super) fn rebuild_listening(&mut self, cx: &Context<Self>) {
        if self.section != Section::Listening {
            return;
        }
        let state = self.discovery.read(cx);
        let mut rows: Vec<ListeningRow> = match self.listening_kind {
            ListeningKind::Artists => state
                .history
                .artists
                .values()
                .filter(|artist| artist.playcount > 0)
                .map(|artist| ListeningRow {
                    artist: artist.name.clone(),
                    album: None,
                    plays: artist.playcount,
                    rank: 0,
                })
                .collect(),
            ListeningKind::Albums => state
                .album_library()
                .into_iter()
                .flat_map(|library| library.artists.iter())
                .flat_map(|(artist, albums)| {
                    let name = state
                        .history
                        .artists
                        .get(artist)
                        .map(|stats| stats.name.as_str())
                        .unwrap_or(artist);
                    albums.values().filter_map(move |album| {
                        let plays = album.count.filter(|count| *count > 0)?;
                        Some(ListeningRow {
                            artist: name.to_owned(),
                            album: Some(album.title.clone()),
                            plays,
                            rank: 0,
                        })
                    })
                })
                .collect(),
        };
        rows.sort_by(|a, b| {
            b.plays
                .cmp(&a.plays)
                .then_with(|| a.artist.cmp(&b.artist))
                .then_with(|| a.album.cmp(&b.album))
        });
        let query = normalize(self.input.read(cx).text());
        self.listening_rows = Rc::new(
            rows.into_iter()
                .enumerate()
                .filter_map(|(index, mut row)| {
                    row.rank = index + 1;
                    normalize(&format!(
                        "{} {}",
                        row.artist,
                        row.album.as_deref().unwrap_or_default()
                    ))
                    .contains(&query)
                    .then_some(row)
                })
                .collect(),
        );
    }

    fn listening_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.listening_rows.get(index) else {
            return div().into_any_element();
        };
        let theme = *cx.theme();
        let lookup = self.catalog_lookup.clone();
        let query = match &row.album {
            Some(title) => CatalogQuery::Album {
                artist: row.artist.clone(),
                title: title.clone(),
            },
            None => CatalogQuery::Artist(row.artist.clone()),
        };
        Card::new(
            ("listening-row", index),
            row.album.as_ref().unwrap_or(&row.artist).clone(),
        )
        .art(theme.metrics.row * 1.5)
        .when(row.album.is_none(), Card::circle)
        .size(Text::Large)
        .weight(FontWeight::BOLD)
        .underline()
        .title_press(move |_, _, cx| lookup.update(cx, |state, cx| state.open(query.clone(), cx)))
        .eyebrow(t!("empath-rank", rank = row.rank))
        .when_some(row.album.as_ref(), |card, _| {
            let lookup = self.catalog_lookup.clone();
            card.bare_meta(
                InlineLinks::new(
                    format!("listening-artist-{index}"),
                    [InlineLink::new(
                        row.artist.clone(),
                        Some(row.artist.clone().into()),
                    )],
                    row.artist.clone(),
                    theme.muted_foreground,
                )
                .text_size(theme.text(Text::Small))
                .on_click(move |artist, cx| {
                    lookup.update(cx, |state, cx| {
                        state.open(CatalogQuery::Artist(artist.to_string()), cx)
                    })
                }),
            )
        })
        .trailing(
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(theme.text(Text::Title))
                        .text_color(theme.primary)
                        .child(row.plays.to_string()),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .text_size(theme.text(Text::Small))
                        .child(t!("listening-plays", count = row.plays)),
                ),
        )
        .h_full()
        .into_any_element()
    }

    /// The same picker handles namesakes, album editions, missing results, and search failures.
    pub(super) fn listening_lookup_modal(&self, cx: &App) -> Option<AnyElement> {
        let state = self.catalog_lookup.read(cx);
        let query = state.query.as_ref()?;
        let name = match query {
            CatalogQuery::Artist(name) => name,
            CatalogQuery::Album { title, .. } => title,
        };
        let theme = *cx.theme();
        let mut body = div()
            .id("listening-matches")
            .flex()
            .flex_col()
            .gap_2()
            .max_h(theme.metrics.row * 8.)
            .overflow_y_scroll();
        if state.loading {
            body = body.child(t!("metal-matching"));
        } else if let Some(error) = state.error {
            body = body.child(i18n::lookup(error, None));
        } else if state.candidates.is_empty() {
            body = body.child(t!("listening-no-match"));
        } else {
            for (index, candidate) in state.candidates.iter().enumerate() {
                let (name, detail, label) = match candidate {
                    CatalogCandidate::Artist(artist) => (
                        artist.name.clone(),
                        artist.id.clone().unwrap_or_default(),
                        "listening-open-artist",
                    ),
                    CatalogCandidate::Album(album) => (
                        album.name.clone(),
                        format!("{} / {}", album.artists, album.release_date),
                        "metal-open-album",
                    ),
                };
                let lookup = self.catalog_lookup.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().font_weight(FontWeight::BOLD).child(name))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(theme.text(Text::Small))
                                        .text_color(theme.muted_foreground)
                                        .child(detail),
                                ),
                        )
                        .child(
                            Button::new(("listening-match-open", index))
                                .small()
                                .label(i18n::lookup(label, None))
                                .on_click(move |_, _, cx| {
                                    lookup.update(cx, |state, cx| state.choose(index, cx))
                                }),
                        ),
                );
            }
        }
        let closed = self.catalog_lookup.clone();
        let dismissed = self.catalog_lookup.clone();
        Some(
            Modal::new(
                "listening-picker",
                t!("listening-lookup-title", name = name.clone()),
            )
            .detail(t!("listening-lookup-help"))
            .child(body)
            .action(
                Button::new("listening-match-close")
                    .label(t!("common-dismiss"))
                    .on_click(move |_, _, cx| closed.update(cx, |state, cx| state.dismiss(cx))),
            )
            .on_dismiss(move |_, _, cx| dismissed.update(cx, |state, cx| state.dismiss(cx)))
            .into_any_element(),
        )
    }

    pub(super) fn listening_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let state = self.discovery.read(cx);
        let account = state.lastfm_user().map(str::to_owned);
        let loading = state.listening_loading
            || match self.listening_kind {
                ListeningKind::Artists => state.history_loading,
                ListeningKind::Albums => state.album_plays_loading,
            };
        let failed = match self.listening_kind {
            ListeningKind::Artists => state.history_error.is_some(),
            ListeningKind::Albums => state.album_plays_failed,
        };
        let updated_at = match self.listening_kind {
            ListeningKind::Artists => state.history.updated_at,
            ListeningKind::Albums => state
                .album_library()
                .map(|library| library.fetched_at)
                .unwrap_or_default(),
        };
        let updated = i64::try_from(updated_at)
            .ok()
            .and_then(|at| jiff::Timestamp::from_second(at).ok())
            .map(|at| {
                at.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            });
        let missing = match self.listening_kind {
            ListeningKind::Artists => state.history.source.is_empty(),
            ListeningKind::Albums => state.album_library().is_none(),
        };
        let header = div()
            .flex()
            .flex_col()
            .gap_4()
            .pb_6()
            .child(
                div()
                    .text_size(theme.text(Text::Display))
                    .font_weight(FontWeight::BOLD)
                    .child(t!("nav-discover")),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(t!("listening-description")),
            )
            .child(self.page_tabs(cx))
            .child(
                TabBar::new("listening-charts").items(
                    [
                        (ListeningKind::Artists, "listening-top-artists"),
                        (ListeningKind::Albums, "listening-top-albums"),
                    ]
                    .into_iter()
                    .map(|(kind, key)| {
                        Button::new(key)
                            .small()
                            .label(i18n::lookup(key, None))
                            .selected(self.listening_kind == kind)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.listening_kind = kind;
                                this.rebuild_listening(cx);
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
                    .gap_3()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(match self.listening_kind {
                        ListeningKind::Artists => {
                            t!("listening-artist-count", count = self.listening_rows.len())
                        }
                        ListeningKind::Albums => {
                            t!("listening-album-count", count = self.listening_rows.len())
                        }
                    })
                    .when_some(account.clone(), |header, user| {
                        header.child(t!("listening-account", user = user))
                    })
                    .when_some(updated.filter(|_| updated_at > 0), |header, updated| {
                        header.child(t!("listening-updated", date = updated))
                    }),
            );
        let rows = self.listening_rows.len();
        Scroller::new("listening-page", &self.scrollbar)
            .px(theme.metrics.inset)
            .py(theme.metrics.inset)
            .max_w(Chrome::content(window, cx))
            .child(header)
            .when(loading, |page| {
                page.child(Notice::new(t!("listening-loading"), t!("listening-cache-help")).mb_4())
            })
            .when(state.listening_failed, |page| {
                page.child(
                    Notice::new(t!("listening-cache-failed"), t!("listening-cache-help"))
                        .failed()
                        .mb_4(),
                )
            })
            .when(failed, |page| {
                let retry = state
                    .album_plays_retry_at
                    .filter(|_| self.listening_kind == ListeningKind::Albums)
                    .map(|at| at.saturating_sub(jiff::Timestamp::now().as_second().max(0) as u64))
                    .filter(|seconds| *seconds > 0);
                let message = match retry {
                    Some(seconds) => t!(
                        "metal-album-plays-rate-limited",
                        minutes = seconds.div_ceil(60)
                    ),
                    None => t!("listening-fetch-failed-help"),
                };
                page.child(
                    Notice::new(t!("listening-fetch-failed"), message)
                        .failed()
                        .mb_4(),
                )
            })
            .when(account.is_none(), |page| {
                page.child(Notice::new(
                    t!("listening-connect"),
                    t!("listening-connect-help"),
                ))
            })
            .when(account.is_some() && !loading && rows == 0, |page| {
                page.child(
                    Notice::new(
                        t!("listening-empty"),
                        match missing {
                            true => t!("listening-missing-help"),
                            false => t!("listening-empty-help"),
                        },
                    )
                    .when(missing, |notice| {
                        notice.action(
                            Button::new("listening-load-history")
                                .label(match self.listening_kind {
                                    ListeningKind::Artists => t!("listening-load-artists"),
                                    ListeningKind::Albums => t!("listening-load-albums"),
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.discovery.update(cx, |state, cx| {
                                        match this.listening_kind {
                                            ListeningKind::Artists => state.load_history(cx),
                                            ListeningKind::Albums => state.load_album_history(cx),
                                        }
                                    })
                                })),
                        )
                    }),
                )
            })
            .child(
                Deck::new("listening-ranked-chart")
                    .rows((0..rows).map(|_| theme.metrics.row * 2. + theme.metrics.pad * 2.))
                    .draw(cx.processor(|this, index, _, cx| this.listening_row(index, cx))),
            )
            .into_any_element()
    }
}
