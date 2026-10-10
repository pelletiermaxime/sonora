use crate::{ArtistStats, Band, BrowserError, History, Release, normalize};
use anyhow::{Context as _, Result, bail};
use scraper::{Html, Selector};
use serde_json::Value;

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("valid static HTML selector")
}
fn text(value: &str) -> String {
    Html::parse_fragment(value)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn cell(row: &Value, index: usize) -> Result<&str> {
    row[index]
        .as_str()
        .context("invalid Metal Archives table cell")
}
fn link(value: &str) -> Result<(String, String, String)> {
    let html = Html::parse_fragment(value);
    let sel = selector("a[href]");
    let anchor = html
        .select(&sel)
        .next()
        .context("Metal Archives cell has no link")?;
    let href = anchor
        .value()
        .attr("href")
        .context("Metal Archives link has no URL")?;
    let url = if href.starts_with('/') {
        format!("https://www.metal-archives.com{href}")
    } else {
        href.to_owned()
    };
    let parsed = reqwest::Url::parse(&url).context("invalid Metal Archives link")?;
    if parsed.host_str() != Some("www.metal-archives.com") || parsed.scheme() != "https" {
        bail!("unexpected Metal Archives link host");
    }
    let id = parsed
        .path_segments()
        .and_then(|mut s| s.rfind(|v| !v.is_empty()))
        .context("Metal Archives link has no ID")?
        .to_owned();
    if !id.chars().all(|c| c.is_ascii_digit()) {
        bail!("Metal Archives link has invalid ID");
    }
    Ok((
        id,
        anchor.text().collect::<Vec<_>>().join("").trim().into(),
        url,
    ))
}

pub(crate) fn band(row: &Value) -> Result<Band> {
    let (id, name, url) = link(cell(row, 0)?)?;
    Ok(Band {
        id,
        name,
        url,
        country: text(row[2].as_str().unwrap_or_default()),
        genre: text(row[1].as_str().unwrap_or_default()),
    })
}

pub(crate) fn upcoming(row: &Value) -> Result<Release> {
    let (id, name, url) = link(cell(row, 0)?)?;
    let band = Band {
        id,
        name,
        url,
        genre: text(cell(row, 3)?),
        country: String::new(),
    };
    let (id, title, url) = link(cell(row, 1)?)?;
    Ok(Release {
        id,
        band,
        title,
        url,
        kind: text(cell(row, 2)?),
        date: date(&text(cell(row, 4)?)),
        review_score: None,
        review_count: 0,
    })
}

/// Reads the identity from a rendered band page, never from unrelated search results.
pub(crate) fn browser_band(value: &str) -> Result<Band> {
    let html = Html::parse_document(value);
    let Some(info) = html.select(&selector("#band_info")).next() else {
        return Err(BrowserError::Incomplete.into());
    };
    let anchor = info
        .select(&selector("h1.band_name a[href]"))
        .next()
        .ok_or(BrowserError::Incomplete)?;
    let (id, name, url) = link(&anchor.html()).map_err(|_| BrowserError::Incomplete)?;
    let mut country = String::new();
    let mut genre = String::new();
    for list in info.select(&selector("dl")) {
        let mut key = String::new();
        for field in list.select(&selector("dt, dd")) {
            let value = text(&field.html());
            match field.value().name() {
                "dt" => key = value,
                "dd" => match key.as_str() {
                    "Country of origin:" => country = value,
                    "Genre:" => genre = value,
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(Band {
        id,
        name,
        url,
        genre,
        country,
    })
}

/// Reads the complete discography captured from the browser's loaded tab.
pub(crate) fn browser_discography(value: &str, band: &Band) -> Result<Vec<Release>> {
    let html = Html::parse_document(value);
    let mut releases = Vec::new();
    let mut found = false;
    for table in html.select(&selector("table.discog")) {
        if table.select(&selector("tbody tr")).next().is_none() {
            continue;
        }
        found = true;
        for release in discography(&table.html(), band)? {
            if !releases
                .iter()
                .any(|known: &Release| known.id == release.id)
            {
                releases.push(release);
            }
        }
    }
    match found {
        true => Ok(releases),
        false => Err(BrowserError::Incomplete.into()),
    }
}

pub(crate) fn discography(value: &str, band: &Band) -> Result<Vec<Release>> {
    let html = Html::parse_document(value);
    let row_selector = selector("tbody tr");
    let cells_selector = selector("td");
    let mut releases = Vec::new();
    if html.select(&selector("table")).next().is_none() {
        bail!("discography response has no table");
    }
    for row in html.select(&row_selector) {
        let cells: Vec<_> = row.select(&cells_selector).collect();
        if cells.len() == 1
            && cells[0]
                .text()
                .collect::<String>()
                .to_lowercase()
                .contains("no")
        {
            continue;
        }
        if cells.len() < 4 {
            bail!("invalid discography row");
        }
        let (id, title, url) = link(&cells[0].html())?;
        let review = text(&cells[3].html());
        let count = review
            .split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let score = review
            .split('(')
            .nth(1)
            .and_then(|s| s.split('%').next())
            .and_then(|s| s.trim().parse().ok());
        releases.push(Release {
            id,
            title,
            url,
            band: band.clone(),
            kind: text(&cells[1].html()),
            date: date(&text(&cells[2].html())),
            review_score: score,
            review_count: count,
        });
    }
    Ok(releases)
}

/// Reads the release date from the album's own information, excluding other editions and reviews.
pub(crate) fn album_date(value: &str) -> Option<String> {
    let html = Html::parse_document(value);
    let info = html.select(&selector("#album_info")).next()?;
    let mut release_date = false;
    for field in info.select(&selector("dl dt, dl dd")) {
        let value = text(&field.html());
        match field.value().name() {
            "dt" => release_date = value == "Release date:",
            "dd" if release_date => return Some(date(&value)),
            _ => {}
        }
    }
    None
}

/// Reads the browser's receipt so unused reserved album lookups can be returned.
pub(crate) fn browser_date_requests(value: &str) -> Option<usize> {
    let html = Html::parse_document(value);
    html.select(&selector("meta[name='sonora-date-requests']"))
        .next()?
        .value()
        .attr("content")?
        .parse()
        .ok()
}

/// Reads a server wait from an album response encountered during browser date checks.
pub(crate) fn browser_date_retry(value: &str) -> Option<String> {
    let html = Html::parse_document(value);
    let value = html
        .select(&selector("meta[name='sonora-date-retry-after']"))
        .next()?
        .value()
        .attr("content")?;
    (!value.is_empty()).then(|| value.to_owned())
}

/// Accepts ISO dates and the English dates used by Metal Archives, preserving imprecise years.
fn date(value: &str) -> String {
    if value.parse::<jiff::civil::Date>().is_ok() {
        return value.into();
    }
    let words: Vec<_> = value.split_whitespace().collect();
    if words.len() == 3 {
        let months = [
            "january",
            "february",
            "march",
            "april",
            "may",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ];
        if let Some(month) = months.iter().position(|m| *m == words[0].to_lowercase()) {
            let day = words[1]
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u8>();
            if let (Ok(day), Ok(year)) = (day, words[2].parse::<i16>())
                && let Ok(date) = jiff::civil::Date::new(year, month as i8 + 1, day as i8)
            {
                return date.to_string();
            }
        }
    }
    value.into()
}

pub(crate) fn seed(text: &str, username: &str) -> Result<History> {
    let value: Value = serde_json::from_str(text).context("cannot decode Last.fm seed")?;
    if let Ok(mut history) = serde_json::from_value::<History>(value.clone()) {
        history.source = format!("seed:{username}");
        history.artists = history
            .artists
            .into_values()
            .map(|s| (normalize(&s.name), s))
            .collect();
        return Ok(history);
    }
    let artists = value
        .as_array()
        .or_else(|| value["artists"].as_array())
        .or_else(|| value["artists"]["artist"].as_array());
    let mut history = History {
        source: format!("seed:{username}"),
        ..History::default()
    };
    if let Some(artists) = artists {
        for artist in artists {
            let stats = crate::stats(artist)?;
            history.artists.insert(normalize(&stats.name), stats);
        }
    } else if let Some(artists) = value.as_object() {
        for (name, count) in artists {
            let playcount = crate::number(count)
                .or_else(|| crate::number(&count["playcount"]))
                .context("invalid Last.fm seed play count")?;
            let name = count["name"].as_str().unwrap_or(name).to_owned();
            history.artists.insert(
                normalize(&name),
                ArtistStats {
                    name,
                    playcount,
                    listeners: None,
                    tags: Vec::new(),
                    url: String::new(),
                },
            );
        }
    } else {
        bail!("unsupported Last.fm seed shape");
    }
    Ok(history)
}
