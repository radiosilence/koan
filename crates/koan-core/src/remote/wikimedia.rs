//! Wikidata, Wikipedia and Wikimedia Commons — where an artist's biography and
//! photograph come from.
//!
//! Reached through the artist's Wikidata item, which MusicBrainz links to. No
//! API key; Wikimedia asks only for a User-Agent that says who is calling.

use serde::Deserialize;
use thiserror::Error;

const USER_AGENT: &str = concat!(
    "koan/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/radiosilence/koan)"
);

/// The article language. English has by far the widest coverage of musicians.
const WIKI: &str = "enwiki";
const WIKIPEDIA_API: &str = "https://en.wikipedia.org/w/api.php";
const COMMONS_API: &str = "https://commons.wikimedia.org/w/api.php";

#[derive(Debug, Error)]
pub enum WikimediaError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

/// What a Wikidata item says about where to read more and what the artist
/// looks like.
#[derive(Debug, Default, PartialEq)]
pub struct Entity {
    /// The English Wikipedia article's title.
    pub article: Option<String>,
    /// A Commons file name (P18, "image").
    pub image: Option<String>,
}

/// An article's lead section, as plain text.
#[derive(Debug, PartialEq)]
pub struct Intro {
    pub text: String,
    pub url: String,
}

/// A Commons image, resized, with the credit its licence asks for.
#[derive(Debug, PartialEq)]
pub struct Image {
    pub url: String,
    pub credit: Option<String>,
}

pub fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

pub fn entity(http: &reqwest::blocking::Client, qid: &str) -> Result<Entity, WikimediaError> {
    let url = format!("https://www.wikidata.org/wiki/Special:EntityData/{qid}.json");
    let body: serde_json::Value = http.get(url).send()?.error_for_status()?.json()?;
    Ok(parse_entity(&body, qid))
}

fn parse_entity(body: &serde_json::Value, qid: &str) -> Entity {
    // A redirected item comes back under its new id, so take whichever is there.
    let item = body["entities"]
        .get(qid)
        .or_else(|| body["entities"].as_object()?.values().next());
    let Some(item) = item else {
        return Entity::default();
    };
    Entity {
        article: item["sitelinks"][WIKI]["title"].as_str().map(String::from),
        image: item["claims"]["P18"][0]["mainsnak"]["datavalue"]["value"]
            .as_str()
            .map(String::from),
    }
}

pub fn intro(
    http: &reqwest::blocking::Client,
    article: &str,
) -> Result<Option<Intro>, WikimediaError> {
    #[derive(Deserialize)]
    struct Response {
        query: Query,
    }
    #[derive(Deserialize)]
    struct Query {
        pages: Vec<Page>,
    }
    #[derive(Deserialize)]
    struct Page {
        extract: Option<String>,
        fullurl: Option<String>,
    }

    let response: Response = http
        .get(WIKIPEDIA_API)
        .query(&[
            ("action", "query"),
            ("prop", "extracts|info"),
            ("inprop", "url"),
            ("exintro", "1"),
            ("explaintext", "1"),
            ("redirects", "1"),
            ("format", "json"),
            ("formatversion", "2"),
            ("titles", article),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(response.query.pages.into_iter().next().and_then(|page| {
        let text = page.extract?.trim().to_string();
        (!text.is_empty()).then(|| Intro {
            text,
            url: page.fullurl.unwrap_or_default(),
        })
    }))
}

pub fn image(
    http: &reqwest::blocking::Client,
    file: &str,
    width: u32,
) -> Result<Option<Image>, WikimediaError> {
    let body: serde_json::Value = http
        .get(COMMONS_API)
        .query(&[
            ("action", "query"),
            ("prop", "imageinfo"),
            ("iiprop", "url|extmetadata"),
            ("iiextmetadatafilter", "Artist|LicenseShortName"),
            ("iiurlwidth", &width.to_string()),
            ("format", "json"),
            ("formatversion", "2"),
            ("titles", &format!("File:{file}")),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(parse_image(&body))
}

fn parse_image(body: &serde_json::Value) -> Option<Image> {
    let info = &body["query"]["pages"][0]["imageinfo"][0];
    let url = info["thumburl"].as_str().or(info["url"].as_str())?;
    let meta = &info["extmetadata"];
    let author = meta["Artist"]["value"]
        .as_str()
        .map(strip_tags)
        .filter(|s| !s.is_empty());
    let licence = meta["LicenseShortName"]["value"]
        .as_str()
        .map(strip_tags)
        .filter(|s| !s.is_empty());
    let credit = match (author, licence) {
        (Some(author), Some(licence)) => Some(format!("{author} · {licence}")),
        (author, licence) => author.or(licence),
    };
    Some(Image {
        url: url.to_string(),
        credit,
    })
}

pub fn download(http: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, WikimediaError> {
    Ok(http.get(url).send()?.error_for_status()?.bytes()?.to_vec())
}

/// Commons credits are HTML — usually a link to the photographer's profile.
fn strip_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entity_reads_the_article_and_the_image() {
        let body = json!({"entities": {"Q2358013": {
            "sitelinks": {"enwiki": {"title": "Glass Candy"}},
            "claims": {"P18": [{"mainsnak": {"datavalue": {
                "value": "Glass Candy Ida No and Johnny Jewel.jpg"
            }}}]}
        }}});
        assert_eq!(
            parse_entity(&body, "Q2358013"),
            Entity {
                article: Some("Glass Candy".into()),
                image: Some("Glass Candy Ida No and Johnny Jewel.jpg".into()),
            }
        );
    }

    #[test]
    fn entity_follows_a_redirected_item() {
        let body = json!({"entities": {"Q2": {"sitelinks": {"enwiki": {"title": "Earth"}}}}});
        assert_eq!(parse_entity(&body, "Q1").article.as_deref(), Some("Earth"));
    }

    #[test]
    fn image_credit_is_plain_text() {
        let body = json!({"query": {"pages": [{"imageinfo": [{
            "thumburl": "https://upload.wikimedia.org/thumb.jpg",
            "extmetadata": {
                "Artist": {"value": "<a rel=\"nofollow\" href=\"https://flickr.com/x\">Jason  Mouratides</a>"},
                "LicenseShortName": {"value": "CC BY 2.0"}
            }
        }]}]}});
        assert_eq!(
            parse_image(&body),
            Some(Image {
                url: "https://upload.wikimedia.org/thumb.jpg".into(),
                credit: Some("Jason Mouratides · CC BY 2.0".into()),
            })
        );
    }

    #[test]
    fn a_missing_file_is_no_image() {
        let body = json!({"query": {"pages": [{"missing": true}]}});
        assert_eq!(parse_image(&body), None);
    }
}
