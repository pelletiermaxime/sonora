use gpui::prelude::*;
use gpui::{AnyElement, App, ElementId, Entity, FontWeight, SharedString, div};
use i18n::t;
use metal::{
    AccessReason, AlbumPlays, BrowserError, EmpathRank, Release, ReleaseAvailability, normalize,
};
use state::{Empath, MetalDiscovery, MetalFetchError, Sonora};
use ui::{ActiveTheme as _, Button, Card, Modal, Text};

pub(crate) use state::metal_release_cover as release_cover;

/// Artist pages show band-level information in the hero; Discover keeps it on each release.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseContext {
    Artist(Option<AlbumPlays>),
    Discover,
}

/// Groups full-length albums separately from singles, EPs, and miscellaneous releases.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscographyTab {
    Main,
    Singles,
    Other,
    All,
}

impl DiscographyTab {
    pub(crate) const GROUPS: [Self; 3] = [Self::Main, Self::Singles, Self::Other];
    pub(crate) const TABS: [Self; 4] = [Self::Main, Self::Singles, Self::Other, Self::All];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Main => "metal-tab-main",
            Self::Singles => "metal-tab-singles",
            Self::Other => "metal-tab-other",
            Self::All => "metal-tab-all",
        }
    }

    pub(crate) fn includes(self, release: &Release) -> bool {
        let group = match release.kind.as_str() {
            "Full-length" => Self::Main,
            "Single" | "EP" => Self::Singles,
            _ => Self::Other,
        };
        self == Self::All || self == group
    }
}

/// Resolves HTTP and browser failures at render time, including real server retry times.
pub(crate) fn fetch_error(error: &MetalFetchError) -> SharedString {
    if let Some(access) = error.access {
        if matches!(access.reason, AccessReason::DailyBudget) {
            let now = jiff::Timestamp::now().as_second().max(0) as u64;
            let hours = access.retry_at.saturating_sub(now).div_ceil(3600).max(1);
            return t!("metal-access-daily-budget", hours = hours);
        }
        let reason = match access.reason {
            AccessReason::Forbidden => "metal-access-forbidden",
            AccessReason::RateLimited => "metal-access-rate-limited",
            AccessReason::Challenge => "metal-access-challenge",
            AccessReason::RetryAfter => "metal-access-retry-after",
            AccessReason::DailyBudget => "metal-access-daily-budget",
            AccessReason::Unknown => "metal-access-forbidden",
        };
        if access.retry_at == 0 {
            return i18n::lookup(reason, None);
        }
        let retry = i64::try_from(access.retry_at)
            .ok()
            .and_then(|seconds| jiff::Timestamp::from_second(seconds).ok())
            .map(|stamp| {
                stamp
                    .to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M %Z")
                    .to_string()
            });
        let until = match retry {
            Some(time) => t!("metal-access-until", time = time),
            None => t!("metal-cache-policy"),
        };
        return format!("{} {}", i18n::lookup(reason, None), until).into();
    }
    let key = match error.browser {
        Some(BrowserError::Unavailable) => "metal-browser-unavailable",
        Some(BrowserError::Closed) => "metal-browser-closed",
        Some(BrowserError::WrongArtist) => "metal-browser-wrong-artist",
        Some(BrowserError::Incomplete) => "metal-browser-incomplete",
        Some(BrowserError::TimedOut) => "metal-browser-timed-out",
        None => "metal-fetch-failed",
    };
    i18n::lookup(key, None)
}

/// An app-aware metadata row. Artwork and streaming matches are never prefetched.
pub(crate) fn release_row(
    id: impl Into<ElementId>,
    release: &Release,
    cover: Option<String>,
    context: ReleaseContext,
    discovery: &Entity<MetalDiscovery>,
    cx: &App,
) -> AnyElement {
    let theme = *cx.theme();
    let state = discovery.read(cx);
    let availability = release.availability(jiff::Zoned::now().date());
    let (plays, info) = match context {
        ReleaseContext::Artist(_) => (0, None),
        ReleaseContext::Discover => (
            state.playcount(&release.band.name),
            state.info.get(&normalize(&release.band.name)),
        ),
    };
    let artist = match plays {
        0 => release.band.name.clone().into(),
        _ => t!(
            "metal-artist-plays",
            artist = release.band.name.clone(),
            count = plays
        ),
    };
    let mut metadata = vec![release.kind.clone()];
    if context == ReleaseContext::Discover && !release.band.genre.is_empty() {
        metadata.push(release.band.genre.clone());
    }
    if !release.band.country.is_empty() {
        metadata.push(release.band.country.clone());
    }
    if !release.date.is_empty() {
        metadata.push(release.date.clone());
    }
    if let ReleaseContext::Artist(Some(counts)) = context {
        metadata.push(match counts.count {
            Some(count) => t!("metal-album-personal-plays", count = count).to_string(),
            None => t!("metal-album-plays-unavailable").to_string(),
        });
    }
    if let Some(listeners) = info.and_then(|info| info.listeners) {
        metadata.push(t!("metal-listeners", count = listeners).to_string());
    }
    if let Some(info) = info
        && !info.tags.is_empty()
    {
        metadata.push(
            info.tags
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let source = release.url.clone();
    let archives = discovery.clone();
    let review_reader = discovery.clone();
    let review_url = release.url.replacen("/albums/", "/reviews/", 1);
    let score = match release.review_score {
        Some(score) => div()
            .id("metal-release-reviews")
            .flex_none()
            .flex()
            .flex_col()
            .items_end()
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                review_reader.update(cx, |state, cx| {
                    state.open_archives(review_url.clone(), window, cx)
                });
            })
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(theme.text(Text::Display))
                    .text_color(theme.primary)
                    .hover(|style| style.underline())
                    .child(t!("metal-score-percent", score = format!("{score:.0}"))),
            )
            .child(
                div()
                    .text_size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground)
                    .child(t!("metal-review-count", count = release.review_count)),
            )
            .into_any_element(),
        None => div()
            .flex_none()
            .text_size(theme.text(Text::Small))
            .text_color(theme.muted_foreground)
            .child(t!("metal-not-reviewed"))
            .into_any_element(),
    };
    let score = match availability {
        ReleaseAvailability::Released => score,
        ReleaseAvailability::Upcoming | ReleaseAvailability::Unconfirmed => {
            let date = release
                .date
                .parse::<jiff::civil::Date>()
                .map(|date| date.strftime("%B %-d, %Y").to_string())
                .unwrap_or_else(|_| release.date.clone());
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_end()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.primary)
                        .child(match availability {
                            ReleaseAvailability::Upcoming => t!("metal-upcoming"),
                            _ => t!("metal-date-unconfirmed"),
                        }),
                )
                .child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(match availability {
                            ReleaseAvailability::Upcoming => t!("metal-release-on", date = date),
                            _ => t!("metal-date-check-help"),
                        }),
                )
                .into_any_element()
        }
    };
    let requested = release.clone();
    let discovery = discovery.clone();
    Card::new(id, release.title.clone())
        .cover(cover.or_else(|| state.cover(release)))
        .art(theme.metrics.row * 1.5)
        .art_radius(theme.radius)
        .hint()
        .underline()
        .weight(match DiscographyTab::Main.includes(release) {
            true => FontWeight::BOLD,
            false => FontWeight::NORMAL,
        })
        .size(match DiscographyTab::Main.includes(release) {
            true => Text::Title,
            false => Text::Body,
        })
        .title_press(move |_, window, cx| {
            archives.update(cx, |state, cx| {
                state.open_archives(source.clone(), window, cx)
            });
        })
        .when(availability == ReleaseAvailability::Released, |card| {
            card.play(false, move |_, _, cx| {
                discovery.update(cx, |state, cx| state.resolve(requested.clone(), cx));
            })
        })
        .w_full()
        .min_w_0()
        .h(theme.metrics.row * 2. + theme.metrics.pad * 2.)
        .py(theme.metrics.pad)
        .border_b_1()
        .border_color(theme.border)
        .bare_meta(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .truncate()
                        .text_color(match plays > 0 {
                            true => theme.primary,
                            false => theme.foreground,
                        })
                        .child(artist),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(metadata.join(" / ")),
                ),
        )
        .when(
            matches!(context, ReleaseContext::Artist(_))
                || release.review_score.is_some()
                || availability != ReleaseAvailability::Released,
            |card| card.trailing(score),
        )
        .into_any_element()
}

/// Shared provider-match picker used by discovery and artist discographies.
pub(crate) fn album_modal(discovery: &Entity<MetalDiscovery>, cx: &App) -> Option<AnyElement> {
    let state = discovery.read(cx);
    let release = state.matching.as_ref()?;
    let theme = *cx.theme();
    let closed = discovery.clone();
    let dismissed = discovery.clone();
    let mut body = div()
        .id("metal-album-results")
        .flex()
        .flex_col()
        .gap_2()
        .max_h(theme.metrics.row * 8.)
        .overflow_y_scroll();
    if state.resolving {
        body = body.child(t!("metal-matching"));
    } else if let Some(error) = state.album_error {
        body = body.child(i18n::lookup(error, None));
    } else if state.albums.is_empty() {
        body = body.child(t!("metal-no-album"));
    } else {
        for (index, album) in state.albums.iter().enumerate() {
            let play = discovery.clone();
            let playing = album.clone();
            let open = discovery.clone();
            let opened = album.id.clone();
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
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .child(album.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme.text(Text::Small))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} / {}", album.artists, album.release_date)),
                            ),
                    )
                    .child(
                        Button::new(("metal-match-open", index))
                            .small()
                            .label(t!("metal-open-album"))
                            .on_click(move |_, _, cx| {
                                open.update(cx, |state, cx| state.dismiss_album(cx));
                                router::navigate(
                                    router::Destination::Album(opened.clone().into()),
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new(("metal-match-play", index))
                            .small()
                            .primary()
                            .icon("icons/play.svg")
                            .label(t!("common-play"))
                            .on_click(move |_, _, cx| {
                                play.update(cx, |state, cx| state.play(playing.clone(), cx))
                            }),
                    ),
            );
        }
    }
    Some(
        Modal::new(
            "metal-album-picker",
            t!("metal-match-title", album = release.title.clone()),
        )
        .detail(t!("metal-match-help", artist = release.band.name.clone()))
        .child(body)
        .action(
            Button::new("metal-match-close")
                .label(t!("common-dismiss"))
                .on_click(move |_, _, cx| closed.update(cx, |state, cx| state.dismiss_album(cx))),
        )
        .on_dismiss(move |_, _, cx| dismissed.update(cx, |state, cx| state.dismiss_album(cx)))
        .into_any_element(),
    )
}

/// A ranked band opens provider matches; the explicit Archives link opens its verified source ID.
pub(crate) fn empath_row(
    id: impl Into<ElementId>,
    rank: &EmpathRank,
    position: usize,
    similar: bool,
    empath: &Entity<Empath>,
    cx: &App,
) -> AnyElement {
    let theme = *cx.theme();
    let find = empath.clone();
    let band = rank.band.clone();
    let archives = Sonora::global(cx).metal.clone();
    let url = band.archives_url();
    let metric = match similar {
        true => t!("empath-similarity", score = format!("{:.3}", rank.score)),
        false => t!("empath-rating", score = format!("{:.1}", rank.score)),
    };
    Card::new(id, band.name.clone())
        .art(theme.metrics.row * 1.5)
        .circle()
        .hint()
        .underline()
        .weight(FontWeight::BOLD)
        .size(Text::Large)
        .eyebrow(t!("empath-rank", rank = position))
        .title_press(move |_, _, cx| find.update(cx, |state, cx| state.find_band(band.clone(), cx)))
        .bare_meta(
            div()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(match similar {
                    true => t!("empath-shared-reviewers", count = rank.reviewers),
                    false => t!("empath-reviewers", count = rank.reviewers),
                }),
        )
        .trailing(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.primary)
                        .child(metric),
                )
                .child(
                    Button::new("empath-band-source")
                        .small()
                        .ghost()
                        .label(t!("metal-source"))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            archives.update(cx, |state, cx| {
                                state.open_archives(url.clone(), window, cx)
                            });
                        }),
                ),
        )
        .w_full()
        .min_w_0()
        .h(theme.metrics.row * 2. + theme.metrics.pad * 2.)
        .py(theme.metrics.pad)
        .border_b_1()
        .border_color(theme.border)
        .into_any_element()
}

/// Keeps attribution and the publisher's snapshot date visible, separate from cache freshness.
pub(crate) fn empath_credit(date: &str, url: String) -> Button {
    Button::new("empath-credit")
        .small()
        .ghost()
        .label(t!("empath-credit", date = date.to_owned()))
        .on_click(move |_, _, cx| cx.open_url(&url))
}

/// Lists matching provider artist identities without silently conflating namesakes.
pub(crate) fn empath_modal(empath: &Entity<Empath>, cx: &App) -> Option<AnyElement> {
    let state = empath.read(cx);
    let band = state.matching.as_ref()?;
    let theme = *cx.theme();
    let closed = empath.clone();
    let dismissed = empath.clone();
    let mut body = div().flex().flex_col().gap_2();
    match (
        state.matching_loading,
        state.matching_failed,
        state.matches.is_empty(),
    ) {
        (true, _, _) => body = body.child(t!("empath-finding-band")),
        (_, true, _) => body = body.child(t!("empath-find-failed")),
        (_, _, true) => body = body.child(t!("empath-no-provider-band")),
        _ => {
            for (index, artist) in state.matches.iter().enumerate() {
                let Some(id) = artist.id.clone() else {
                    continue;
                };
                let picked = empath.clone();
                body = body.child(
                    Button::new(("empath-artist-match", index))
                        .outline()
                        .label(t!(
                            "empath-provider-match",
                            artist = artist.name.clone(),
                            id = id.clone()
                        ))
                        .on_click(move |_, _, cx| {
                            if picked.update(cx, |state, cx| state.choose_match(&id, cx)) {
                                router::navigate(
                                    router::Destination::Artist(id.clone().into()),
                                    cx,
                                );
                            }
                        }),
                );
            }
        }
    }
    Some(
        Modal::new(
            "empath-artist-picker",
            t!("empath-find-title", artist = band.name.clone()),
        )
        .detail(t!("empath-find-help"))
        .child(
            body.max_h(theme.metrics.row * 8.)
                .id("empath-matches")
                .overflow_y_scroll(),
        )
        .action(
            Button::new("empath-match-close")
                .label(t!("common-dismiss"))
                .on_click(move |_, _, cx| closed.update(cx, |state, cx| state.dismiss_match(cx))),
        )
        .on_dismiss(move |_, _, cx| dismissed.update(cx, |state, cx| state.dismiss_match(cx)))
        .into_any_element(),
    )
}

pub(crate) fn notice(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .py_3()
        .text_size(cx.theme().text(Text::Small))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}
