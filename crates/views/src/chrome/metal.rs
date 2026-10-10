use gpui::prelude::*;
use gpui::{AnyElement, App, ElementId, Entity, FontWeight, SharedString, div};
use i18n::t;
use metal::{Release, normalize};
use state::MetalDiscovery;
use ui::{ActiveTheme as _, Button, Modal, Text};

/// An app-aware metadata row. Artwork and streaming matches are never prefetched.
pub(crate) fn release_row(
    id: impl Into<ElementId>,
    release: &Release,
    discovery: &Entity<MetalDiscovery>,
    cx: &App,
) -> AnyElement {
    let theme = *cx.theme();
    let state = discovery.read(cx);
    let plays = state.playcount(&release.band.name);
    let info = state.info.get(&normalize(&release.band.name));
    let artist = match plays {
        0 => release.band.name.clone().into(),
        _ => t!(
            "metal-artist-plays",
            artist = release.band.name.clone(),
            count = plays
        ),
    };
    let mut metadata = vec![release.kind.clone()];
    if !release.band.genre.is_empty() {
        metadata.push(release.band.genre.clone());
    }
    if !release.band.country.is_empty() {
        metadata.push(release.band.country.clone());
    }
    if !release.date.is_empty() {
        metadata.push(release.date.clone());
    }
    if let Some(score) = release.review_score {
        metadata.push(
            t!(
                "metal-review-score",
                score = format!("{score:.0}"),
                count = release.review_count
            )
            .to_string(),
        );
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
    let requested = release.clone();
    let discovery = discovery.clone();
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap_3()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .child(release.title.clone()),
                )
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
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Button::new("metal-release-play")
                        .small()
                        .icon("icons/play.svg")
                        .label(t!("common-play"))
                        .on_click(move |_, _, cx| {
                            discovery.update(cx, |state, cx| state.resolve(requested.clone(), cx));
                        }),
                )
                .child(
                    Button::new("metal-release-source")
                        .small()
                        .ghost()
                        .label(t!("metal-source"))
                        .on_click(move |_, _, cx| cx.open_url(&source)),
                ),
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
    } else if let Some(error) = &state.album_error {
        body = body.child(error.clone());
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

pub(crate) fn notice(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .py_3()
        .text_size(cx.theme().text(Text::Small))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}
