//! Dataset information for a catalog, from NOAA InPort or any ISO 19115 / 19115-2 record
//! (ISO 19139 XML, as InPort exports at /inport/item/<id>/iso19115). Read-only: the
//! catalog's OASIS JSON never changes; this goes into the detailed export and saved catalog.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use roxmltree::{Document, Node};
use serde::Serialize;

pub const INPORT_ITEM: &str = "https://www.fisheries.noaa.gov/inport/item/";
const MAX_XML_BYTES: usize = 10 * 1024 * 1024;
const MAX_LIST: usize = 50;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct BoundingBox {
    pub west: f64,
    pub east: f64,
    pub south: f64,
    pub north: f64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Contact {
    pub name: Option<String>,
    pub organization: Option<String>,
    pub role: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Identifier {
    pub code: String,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DatasetInfo {
    /// Where it came from: the InPort item, a URL or a file name.
    pub source: String,
    pub inport_id: Option<String>,
    pub inport_url: Option<String>,
    pub file_identifier: Option<String>,
    pub title: Option<String>,
    pub alternate_title: Option<String>,
    #[serde(rename = "abstract")]
    pub abstract_text: Option<String>,
    pub purpose: Option<String>,
    pub publication_date: Option<String>,
    pub keywords: Vec<String>,
    pub topic_categories: Vec<String>,
    pub bounding_box: Option<BoundingBox>,
    pub time_begin: Option<String>,
    pub time_end: Option<String>,
    pub contacts: Vec<Contact>,
    pub use_limitations: Vec<String>,
    pub other_constraints: Vec<String>,
    pub identifiers: Vec<Identifier>,
    /// A DOI among the identifiers, as a link.
    pub doi: Option<String>,
}

fn named<'a, 'i, 'n>(node: Node<'a, 'i>, name: &'n str) -> impl Iterator<Item = Node<'a, 'i>> + 'n
where
    'a: 'n,
    'i: 'n,
{
    node.descendants().filter(move |n| n.is_element() && n.tag_name().name() == name)
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == name)
}

/// The text of an ISO property: its CharacterString / Anchor / Decimal / Date child, or itself.
fn text(node: Node) -> Option<String> {
    let inner = node.children().find(|n| n.is_element()).filter(|n| matches!(n.tag_name().name(), "CharacterString" | "Anchor" | "Decimal" | "Date" | "DateTime" | "Real" | "Integer" | "URL")).unwrap_or(node);
    let joined: String = inner.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect();
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn first_text(node: Node, name: &str) -> Option<String> {
    named(node, name).find_map(text)
}

fn code_value(node: Node) -> Option<String> {
    let code = node.descendants().find(|n| n.is_element() && n.attribute("codeListValue").is_some())?;
    code.attribute("codeListValue").map(str::to_string).filter(|s| !s.is_empty())
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if list.len() < MAX_LIST && !list.contains(&value) {
        list.push(value);
    }
}

/// Read an ISO 19115 / 19115-2 record (ISO 19139 XML, any namespace prefixes).
pub fn parse_iso(xml: &str, source: &str) -> Result<DatasetInfo> {
    let doc = Document::parse(xml).context("not valid XML")?;
    let root = doc.root_element();
    if !matches!(root.tag_name().name(), "MI_Metadata" | "MD_Metadata") {
        bail!("not an ISO 19115 metadata record (the document starts with <{}>)", root.tag_name().name());
    }
    let ident = named(root, "MD_DataIdentification").next().or_else(|| named(root, "SV_ServiceIdentification").next()).unwrap_or(root);
    let citation = child(ident, "citation").and_then(|c| child(c, "CI_Citation"));
    let mut info = DatasetInfo { source: source.to_string(), file_identifier: child(root, "fileIdentifier").and_then(text), ..Default::default() };
    if let Some(cit) = citation {
        info.title = child(cit, "title").and_then(text);
        info.alternate_title = child(cit, "alternateTitle").and_then(text);
        for date in named(cit, "CI_Date") {
            if child(date, "dateType").and_then(code_value).as_deref() == Some("publication") {
                info.publication_date = child(date, "date").and_then(text);
            }
        }
        for id in named(cit, "MD_Identifier") {
            let Some(code) = child(id, "code") else { continue };
            let Some(value) = text(code) else { continue };
            let link = code.descendants().find_map(|n| n.attributes().find(|a| a.name() == "href").map(|a| a.value().to_string()));
            if info.identifiers.len() < MAX_LIST {
                info.identifiers.push(Identifier { code: value, link });
            }
        }
    }
    info.abstract_text = child(ident, "abstract").and_then(text);
    info.purpose = child(ident, "purpose").and_then(text);
    for keyword in named(ident, "keyword").filter_map(text) {
        push_unique(&mut info.keywords, keyword);
    }
    for topic in named(ident, "MD_TopicCategoryCode").filter_map(text) {
        push_unique(&mut info.topic_categories, topic);
    }
    if let Some(bbox) = named(ident, "EX_GeographicBoundingBox").next() {
        let n = |name| child(bbox, name).and_then(text).and_then(|t| t.parse::<f64>().ok());
        if let (Some(west), Some(east), Some(south), Some(north)) = (n("westBoundLongitude"), n("eastBoundLongitude"), n("southBoundLatitude"), n("northBoundLatitude")) {
            info.bounding_box = Some(BoundingBox { west, east, south, north });
        }
    }
    if let Some(period) = named(ident, "TimePeriod").next() {
        let position = |name| {
            let node = child(period, name)?;
            text(node).or_else(|| node.attribute("indeterminatePosition").map(str::to_string))
        };
        info.time_begin = position("beginPosition");
        info.time_end = position("endPosition");
    } else if let Some(instant) = named(ident, "TimeInstant").next() {
        info.time_begin = first_text(instant, "timePosition");
        info.time_end = info.time_begin.clone();
    }
    for party in named(root, "CI_ResponsibleParty").chain(named(root, "CI_Responsibility")) {
        let contact = Contact {
            name: child(party, "individualName").and_then(text).or_else(|| named(party, "CI_Individual").next().and_then(|i| first_text(i, "name"))),
            organization: child(party, "organisationName").and_then(text).or_else(|| named(party, "CI_Organisation").next().and_then(|o| child(o, "name")).and_then(text)),
            role: child(party, "role").and_then(code_value),
            email: first_text(party, "electronicMailAddress"),
        };
        if (contact.name.is_some() || contact.organization.is_some()) && !info.contacts.contains(&contact) && info.contacts.len() < MAX_LIST {
            info.contacts.push(contact);
        }
    }
    for constraint in named(ident, "useLimitation").filter_map(text) {
        push_unique(&mut info.use_limitations, constraint);
    }
    for constraint in named(ident, "otherConstraints").filter_map(text) {
        push_unique(&mut info.other_constraints, constraint);
    }
    info.doi = info.identifiers.iter().find_map(|id| {
        let code = id.code.trim();
        let lower = code.to_lowercase();
        if lower.starts_with("10.") {
            Some(format!("https://doi.org/{code}"))
        } else if lower.starts_with("doi:") {
            Some(format!("https://doi.org/{}", code[4..].trim()))
        } else if lower.contains("doi.org/") {
            Some(code.to_string())
        } else {
            id.link.as_ref().filter(|l| l.contains("doi.org/")).cloned()
        }
    });
    // InPort records name themselves (gov.noaa.nmfs.inport:27377).
    if let Some(id) = info.file_identifier.as_deref().and_then(|f| f.strip_prefix("gov.noaa.nmfs.inport:")) {
        info.inport_id = Some(id.to_string());
        info.inport_url = Some(format!("{INPORT_ITEM}{id}"));
    }
    Ok(info)
}

/// The ISO record URL for an InPort catalog item (a number, or its page URL).
pub fn inport_iso_url(item: &str) -> Result<String> {
    let item = item.trim().trim_end_matches('/');
    let id = item.rsplit('/').find(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    match id {
        Some(id) => Ok(format!("{INPORT_ITEM}{id}/iso19115")),
        None => bail!("Enter an InPort catalog item number, such as 27377, or its page address"),
    }
}

/// Fetch and read a record from a URL (an InPort item number works too).
pub async fn fetch(item_or_url: &str) -> Result<DatasetInfo> {
    let text = item_or_url.trim();
    let url = if text.starts_with("http://") || text.starts_with("https://") {
        if text.contains("/inport/item/") && !text.contains("/iso19115") { inport_iso_url(text)? } else { text.to_string() }
    } else {
        inport_iso_url(text)?
    };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).user_agent("JetStream-Desktop").build()?;
    let response = client.get(&url).send().await.with_context(|| format!("couldn't reach {url}"))?;
    if !response.status().is_success() {
        bail!("{url} answered {}", response.status());
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_XML_BYTES {
        bail!("{url} is over 10 MB; that's not a metadata record");
    }
    let xml = String::from_utf8_lossy(&bytes);
    let mut info = parse_iso(&xml, &url).map_err(|e| anyhow!("{url}: {e:#}"))?;
    if info.inport_id.is_none() && url.starts_with(INPORT_ITEM) {
        let id = url[INPORT_ITEM.len()..].split('/').next().unwrap_or_default().to_string();
        info.inport_url = Some(format!("{INPORT_ITEM}{id}"));
        info.inport_id = Some(id);
    }
    Ok(info)
}

/// Read a record from a file on this computer.
pub fn read_file(path: &std::path::Path) -> Result<DatasetInfo> {
    let meta = std::fs::metadata(path).with_context(|| format!("can't open {}", path.display()))?;
    if meta.len() as usize > MAX_XML_BYTES {
        bail!("{} is over 10 MB; that's not a metadata record", path.display());
    }
    let xml = std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
    parse_iso(&xml, &path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the shape of an InPort ISO 19115-2 export.
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gmi:MI_Metadata xmlns:gco="http://www.isotc211.org/2005/gco" xmlns:gmd="http://www.isotc211.org/2005/gmd"
    xmlns:gmi="http://www.isotc211.org/2005/gmi" xmlns:gml="http://www.opengis.net/gml/3.2"
    xmlns:gmx="http://www.isotc211.org/2005/gmx" xmlns:xlink="http://www.w3.org/1999/xlink">
  <gmd:fileIdentifier><gco:CharacterString>gov.noaa.nmfs.inport:12345</gco:CharacterString></gmd:fileIdentifier>
  <gmd:contact><gmd:CI_ResponsibleParty>
    <gmd:individualName><gco:CharacterString>Jane Diver</gco:CharacterString></gmd:individualName>
    <gmd:contactInfo><gmd:CI_Contact><gmd:address><gmd:CI_Address>
      <gmd:electronicMailAddress><gco:CharacterString>jane.diver@example.gov</gco:CharacterString></gmd:electronicMailAddress>
    </gmd:CI_Address></gmd:address></gmd:CI_Contact></gmd:contactInfo>
    <gmd:role><gmd:CI_RoleCode codeList="x" codeListValue="pointOfContact">pointOfContact</gmd:CI_RoleCode></gmd:role>
  </gmd:CI_ResponsibleParty></gmd:contact>
  <gmd:identificationInfo><gmd:MD_DataIdentification>
    <gmd:citation><gmd:CI_Citation>
      <gmd:title><gco:CharacterString>Benthic imagery, Main Hawaiian Islands</gco:CharacterString></gmd:title>
      <gmd:date><gmd:CI_Date><gmd:date><gco:Date>2024</gco:Date></gmd:date>
        <gmd:dateType><gmd:CI_DateTypeCode codeList="x" codeListValue="publication">publication</gmd:CI_DateTypeCode></gmd:dateType></gmd:CI_Date></gmd:date>
      <gmd:identifier><gmd:MD_Identifier><gmd:code>
        <gmx:Anchor xlink:href="https://www.fisheries.noaa.gov/inport/item/12345" xlink:title="InPort Catalog ID">12345</gmx:Anchor>
      </gmd:code></gmd:MD_Identifier></gmd:identifier>
      <gmd:identifier><gmd:MD_Identifier><gmd:code><gco:CharacterString>doi:10.7289/V5ABCDEF</gco:CharacterString></gmd:code></gmd:MD_Identifier></gmd:identifier>
    </gmd:CI_Citation></gmd:citation>
    <gmd:abstract><gco:CharacterString>Downward-facing photos from towed-diver surveys.</gco:CharacterString></gmd:abstract>
    <gmd:purpose><gco:CharacterString>Coral cover estimates.</gco:CharacterString></gmd:purpose>
    <gmd:descriptiveKeywords><gmd:MD_Keywords>
      <gmd:keyword><gco:CharacterString>coral reef</gco:CharacterString></gmd:keyword>
      <gmd:keyword><gmx:Anchor xlink:href="x">EARTH SCIENCE &gt; OCEANS</gmx:Anchor></gmd:keyword>
      <gmd:keyword><gco:CharacterString>coral reef</gco:CharacterString></gmd:keyword>
    </gmd:MD_Keywords></gmd:descriptiveKeywords>
    <gmd:resourceConstraints><gmd:MD_LegalConstraints>
      <gmd:useLimitation><gco:CharacterString>Cite the dataset.</gco:CharacterString></gmd:useLimitation>
      <gmd:otherConstraints><gco:CharacterString>Distribution liability: none.</gco:CharacterString></gmd:otherConstraints>
    </gmd:MD_LegalConstraints></gmd:resourceConstraints>
    <gmd:topicCategory><gmd:MD_TopicCategoryCode>oceans</gmd:MD_TopicCategoryCode></gmd:topicCategory>
    <gmd:extent><gmd:EX_Extent>
      <gmd:geographicElement><gmd:EX_GeographicBoundingBox>
        <gmd:westBoundLongitude><gco:Decimal>-160.5</gco:Decimal></gmd:westBoundLongitude>
        <gmd:eastBoundLongitude><gco:Decimal>-154.7</gco:Decimal></gmd:eastBoundLongitude>
        <gmd:southBoundLatitude><gco:Decimal>18.9</gco:Decimal></gmd:southBoundLatitude>
        <gmd:northBoundLatitude><gco:Decimal>22.3</gco:Decimal></gmd:northBoundLatitude>
      </gmd:EX_GeographicBoundingBox></gmd:geographicElement>
      <gmd:temporalElement><gmd:EX_TemporalExtent><gmd:extent>
        <gml:TimePeriod gml:id="t1"><gml:beginPosition>2013-04-01</gml:beginPosition><gml:endPosition indeterminatePosition="now"/></gml:TimePeriod>
      </gmd:extent></gmd:EX_TemporalExtent></gmd:temporalElement>
    </gmd:EX_Extent></gmd:extent>
  </gmd:MD_DataIdentification></gmd:identificationInfo>
</gmi:MI_Metadata>"#;

    #[test]
    fn reads_an_inport_iso_record() {
        let info = parse_iso(SAMPLE, "test").unwrap();
        assert_eq!(info.title.as_deref(), Some("Benthic imagery, Main Hawaiian Islands"));
        assert_eq!(info.inport_id.as_deref(), Some("12345"));
        assert_eq!(info.inport_url.as_deref(), Some("https://www.fisheries.noaa.gov/inport/item/12345"));
        assert_eq!(info.publication_date.as_deref(), Some("2024"));
        assert_eq!(info.keywords, vec!["coral reef", "EARTH SCIENCE > OCEANS"]);
        assert_eq!(info.topic_categories, vec!["oceans"]);
        assert_eq!(info.bounding_box, Some(BoundingBox { west: -160.5, east: -154.7, south: 18.9, north: 22.3 }));
        assert_eq!((info.time_begin.as_deref(), info.time_end.as_deref()), (Some("2013-04-01"), Some("now")));
        assert_eq!(info.contacts.len(), 1);
        assert_eq!(info.contacts[0].email.as_deref(), Some("jane.diver@example.gov"));
        assert_eq!(info.contacts[0].role.as_deref(), Some("pointOfContact"));
        assert_eq!(info.identifiers[0].link.as_deref(), Some("https://www.fisheries.noaa.gov/inport/item/12345"));
        assert_eq!(info.doi.as_deref(), Some("https://doi.org/10.7289/V5ABCDEF"));
        assert_eq!(info.use_limitations, vec!["Cite the dataset."]);
        assert_eq!(info.abstract_text.as_deref(), Some("Downward-facing photos from towed-diver surveys."));
    }

    #[test]
    fn rejects_other_documents_and_parses_item_numbers() {
        assert!(parse_iso("<html><body/></html>", "x").is_err());
        assert!(parse_iso("not xml", "x").is_err());
        assert_eq!(inport_iso_url("27377").unwrap(), "https://www.fisheries.noaa.gov/inport/item/27377/iso19115");
        assert_eq!(inport_iso_url("https://www.fisheries.noaa.gov/inport/item/27377").unwrap(), "https://www.fisheries.noaa.gov/inport/item/27377/iso19115");
        assert!(inport_iso_url("reef").is_err());
    }
}
