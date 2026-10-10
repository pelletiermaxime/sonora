use anyhow::{Context as _, Result, bail};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::future::Future;

use crate::{Client, DAY, normalize, now};

const BASE: &str = "https://www.furia.com/em/";

/// An empath response retaining its source cache deadline, including stale refresh fallbacks.
pub struct EmpathCached<T> {
    pub data: T,
    pub expired: bool,
    /// Unix seconds when this response becomes stale; zero requires revalidation.
    pub fresh_until: u64,
}

/// An empath band identified by its Metal Archives ID, independent of streaming providers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmpathBand {
    pub id: String,
    pub name: String,
}

impl EmpathBand {
    pub fn archives_url(&self) -> String {
        format!("https://www.metal-archives.com/band/view/id/{}", self.id)
    }
}

/// The source's rating score or similarity coefficient, with its contributing reviewer count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmpathRank {
    pub band: EmpathBand,
    pub score: f32,
    pub reviewers: u64,
}

/// One style's bands in the source's ranking order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmpathStyle {
    pub name: String,
    pub bands: Vec<EmpathRank>,
}

/// The complete style ranking page and its publisher's update date.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmpathGenres {
    pub updated: String,
    pub styles: Vec<EmpathStyle>,
}

impl EmpathGenres {
    /// Parses the complete style page and rejects missing groups or truncated band lists.
    pub fn from_html(html: &str) -> Result<Self> {
        parse_genres(html)
    }
}

/// Similar bands from shared reviewers, preserving the verified source identity and date.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmpathSimilar {
    pub updated: String,
    pub band: EmpathBand,
    pub bands: Vec<EmpathRank>,
}

impl EmpathSimilar {
    /// Parses only the Similar Bands section after verifying the page's Archives identity.
    pub fn from_html(id: &str, html: &str) -> Result<Self> {
        valid_id(id)?;
        parse_similar(id, html)
    }
}

/// A direct HTTP client for empath, with an independent 24-hour cache and no Archives requests.
#[derive(Clone)]
pub struct EmpathClient {
    client: Client,
}

impl EmpathClient {
    pub fn new(cache: storage::Cache) -> Self {
        Self {
            client: Client::new(cache),
        }
    }

    /// Loads the single page containing all styles, retaining saved data if a refresh fails.
    pub async fn genres(&self, refresh: bool) -> Result<EmpathCached<EmpathGenres>> {
        self.load("empath:genres:v1", refresh, async {
            EmpathGenres::from_html(&self.page("bands-style.html").await?)
        })
        .await
    }

    /// Uses an Archives identity when known; otherwise resolves only unique exact name matches.
    pub async fn identities(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> Result<EmpathCached<Vec<EmpathBand>>> {
        if let Some(id) = id {
            valid_id(id)?;
            return Ok(EmpathCached {
                data: vec![EmpathBand {
                    id: id.to_owned(),
                    name: name.to_owned(),
                }],
                expired: false,
                fresh_until: now().saturating_add(DAY),
            });
        }
        let index = self
            .load("empath:index:v1", false, async {
                let doc = Html::parse_document(&self.page("index.html").await?);
                let bands = doc
                    .select(&selector(".hsim a[href]"))
                    .map(band)
                    .collect::<Result<Vec<_>>>()?;
                if bands.is_empty() {
                    bail!("empath index has no bands");
                }
                Ok(bands)
            })
            .await?;
        Ok(EmpathCached {
            data: index
                .data
                .into_iter()
                .filter(|band| normalize(&band.name) == normalize(name))
                .collect(),
            expired: index.expired,
            fresh_until: index.fresh_until,
        })
    }

    /// Loads one band page by Archives ID and exports only its Similar Bands table.
    pub async fn similar(&self, id: &str, refresh: bool) -> Result<EmpathCached<EmpathSimilar>> {
        valid_id(id)?;
        self.load(&format!("empath:similar:v1:{id}"), refresh, async {
            EmpathSimilar::from_html(id, &self.page(&format!("band{id}.html")).await?)
        })
        .await
    }

    async fn page(&self, path: &str) -> Result<String> {
        self.client
            .http
            .get(format!("{BASE}{path}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await
            .context("cannot read empath page")
    }

    /// Coalesces requests and preserves the last parsed snapshot without extending its age.
    async fn load<T: Serialize + DeserializeOwned>(
        &self,
        key: &str,
        refresh: bool,
        fetch: impl Future<Output = Result<T>>,
    ) -> Result<EmpathCached<T>> {
        if !refresh
            && let Some(saved) = self.client.snapshot::<T>(key)?
            && now().saturating_sub(saved.at) < DAY
        {
            return Ok(EmpathCached {
                data: saved.value,
                expired: false,
                fresh_until: saved.at.saturating_add(DAY),
            });
        }
        let _operation = self.client.operation.lock().await;
        let saved = self.client.snapshot::<T>(key)?;
        if !refresh
            && saved
                .as_ref()
                .is_some_and(|saved| now().saturating_sub(saved.at) < DAY)
        {
            let saved = saved.expect("fresh snapshot exists");
            return Ok(EmpathCached {
                data: saved.value,
                expired: false,
                fresh_until: saved.at.saturating_add(DAY),
            });
        }
        match fetch.await {
            Ok(data) => {
                let fresh_until = now().saturating_add(DAY);
                self.client.save(key, &data)?;
                Ok(EmpathCached {
                    data,
                    expired: false,
                    fresh_until,
                })
            }
            Err(error) => match saved {
                Some(saved) => {
                    log::warn!("empath: cannot update saved data: {error:#}");
                    Ok(EmpathCached {
                        data: saved.value,
                        expired: true,
                        fresh_until: 0,
                    })
                }
                None => Err(error),
            },
        }
    }
}

/// Rejects a malformed or partial ranking page instead of replacing the saved snapshot.
fn parse_genres(html: &str) -> Result<EmpathGenres> {
    let doc = Html::parse_document(html);
    let mut styles: Vec<EmpathStyle> = Vec::new();
    let mut totals = Vec::new();
    for row in doc.select(&selector("table tr")) {
        if let Some(heading) = row.select(&selector("td.groupheading")).next() {
            let name = heading
                .select(&selector("a[name]"))
                .next()
                .context("empath style has no name")?;
            let total = heading
                .select(&selector(".groupcount"))
                .next()
                .context("empath style has no total")?;
            totals.push(text(total).parse::<usize>()?);
            styles.push(EmpathStyle {
                name: text(name),
                bands: Vec::new(),
            });
        } else if let Some(style) = styles.last_mut() {
            style.bands.push(rank(row)?);
        }
    }
    if styles.is_empty()
        || styles
            .iter()
            .zip(totals)
            .any(|(style, total)| style.bands.len() != total)
    {
        bail!("empath style rankings are incomplete");
    }
    Ok(EmpathGenres {
        updated: updated(&doc)?,
        styles,
    })
}

/// Verifies the page's Encyclopaedia link and excludes releases, reviewers, and other tables.
fn parse_similar(id: &str, html: &str) -> Result<EmpathSimilar> {
    let doc = Html::parse_document(html);
    let heading = doc
        .select(&selector("div.band"))
        .next()
        .context("empath band has no heading")?;
    let link = heading
        .select(&selector("a[href]"))
        .next()
        .context("empath band has no Archives link")?;
    let url = reqwest::Url::parse(link.value().attr("href").unwrap_or_default())?;
    if url.host_str() != Some("www.metal-archives.com")
        || url.path() != format!("/band/view/id/{id}")
    {
        bail!("empath band identity does not match requested Archives ID");
    }
    let name = heading
        .children()
        .filter_map(|node| node.value().as_text())
        .map(|text| text.to_string())
        .collect::<String>()
        .trim()
        .to_owned();
    let table = doc
        .select(&selector("table"))
        .find(|table| table.select(&selector("#similars")).next().is_some())
        .context("empath band has no similar bands table")?;
    let mut bands = Vec::new();
    let mut similar_rows = false;
    for row in table.select(&selector("tr")) {
        if let Some(heading) = row.select(&selector(".groupheading")).next() {
            if similar_rows {
                break;
            }
            similar_rows = heading.value().attr("id") == Some("similars");
        } else if similar_rows {
            let rank = rank(row)?;
            if rank.score > 1.0 {
                bail!("empath similarity coefficient is invalid");
            }
            bands.push(rank);
        }
    }
    Ok(EmpathSimilar {
        updated: updated(&doc)?,
        band: EmpathBand {
            id: id.to_owned(),
            name,
        },
        bands,
    })
}

/// Reads exactly the score, reviewer count, and band link from a source table row.
fn rank(row: ElementRef<'_>) -> Result<EmpathRank> {
    let cells = row
        .select(&selector("td.stat"))
        .map(text)
        .collect::<Vec<_>>();
    if cells.len() != 2 {
        bail!("empath rank has incomplete statistics");
    }
    let score: f32 = cells[0].parse()?;
    if !score.is_finite() || !(0.0..=100.0).contains(&score) {
        bail!("empath rank has an invalid score");
    }
    Ok(EmpathRank {
        band: band(
            row.select(&selector("td.val a[href]"))
                .next()
                .context("empath rank has no band")?,
        )?,
        score,
        reviewers: cells[1].parse()?,
    })
}

/// Converts an empath link to an Archives ID while retaining the source's band name.
fn band(anchor: ElementRef<'_>) -> Result<EmpathBand> {
    let url = reqwest::Url::parse(BASE)?.join(anchor.value().attr("href").unwrap_or_default())?;
    if url.host_str() != Some("www.furia.com") {
        bail!("empath band link has an unexpected host");
    }
    let id = url
        .path()
        .strip_prefix("/em/band")
        .and_then(|id| id.strip_suffix(".html"))
        .context("empath band link has no Archives ID")?;
    valid_id(id)?;
    Ok(EmpathBand {
        id: id.to_owned(),
        name: text(anchor),
    })
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("invalid empath Archives ID");
    }
    Ok(())
}

/// Keeps the publisher's snapshot date rather than substituting the current fetch date.
fn updated(doc: &Html) -> Result<String> {
    let credits = doc
        .select(&selector(".credits"))
        .next()
        .context("empath page has no credits")?;
    text(credits)
        .rsplit_once("Updated ")
        .map(|(_, date)| date.trim().to_owned())
        .filter(|date| !date.is_empty())
        .context("empath page has no update date")
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("valid empath selector")
}

fn text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}
