//! Structure-preserving text extraction for PDF, Office, Markdown, HTML, web pages and plain text.
//! Everything is parsed in-process (no external converters).

use super::store::DocKind;
use anyhow::{anyhow, bail, Context, Result};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: u8, text: String },
    Para(String),
    /// Page / slide boundary (1-based).
    Page(u32),
}

#[derive(Clone, Debug)]
pub struct Extracted {
    pub title: String,
    pub kind: DocKind,
    pub blocks: Vec<Block>,
}

impl Extracted {
    pub fn word_count(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| match b {
                Block::Heading { text, .. } | Block::Para(text) => text.split_whitespace().count(),
                Block::Page(_) => 0,
            })
            .sum()
    }
}

pub fn is_image_extension(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff")
}

pub fn supported_extension(ext: &str) -> bool {
    is_image_extension(ext)
        || crate::audio::decode::supported_extension(ext)
        || matches!(
            ext.to_ascii_lowercase().as_str(),
            "pdf" | "docx" | "pptx" | "xlsx" | "odt" | "odp" | "ods" | "epub" | "rtf" | "md" | "markdown" | "txt" | "text" | "html" | "htm" | "xhtml" | "csv" | "tsv" | "json" | "xml" | "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "kt" | "c" | "cpp" | "cc" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "sh" | "sql" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "rst" | "org" | "tex" | "log" | "srt" | "vtt"
        )
}

pub fn extract_path(path: &Path) -> Result<Extracted> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Document").to_string();
    match ext.as_str() {
        "pdf" => extract_pdf(path, &stem),
        "docx" => extract_docx(path, &stem),
        "pptx" => extract_pptx(path, &stem),
        "xlsx" => extract_xlsx(path, &stem),
        "odt" | "odp" | "ods" => extract_opendocument(path, &stem, &ext),
        "epub" => extract_epub(path, &stem),
        "rtf" => {
            let bytes = std::fs::read(path)?;
            Ok(extract_rtf(&String::from_utf8_lossy(&bytes), &stem))
        }
        "srt" | "vtt" => {
            let text = std::fs::read_to_string(path)?;
            Ok(extract_subtitles(&text, &stem))
        }
        "md" | "markdown" => {
            let text = std::fs::read_to_string(path)?;
            Ok(extract_markdown(&text, &stem))
        }
        "html" | "htm" => {
            let html = std::fs::read_to_string(path)?;
            Ok(extract_html(&html, &stem, DocKind::Html))
        }
        "csv" => {
            let text = std::fs::read_to_string(path)?;
            Ok(extract_table_text(&text, &stem))
        }
        _ => {
            let bytes = std::fs::read(path)?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            let kind = if matches!(ext.as_str(), "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "kt" | "c" | "cpp" | "cc" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "sh" | "sql" | "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf") {
                DocKind::Code
            } else {
                DocKind::Text
            };
            Ok(extract_plain(&text, &stem, kind))
        }
    }
}

pub fn fetch_url(url: &str) -> Result<Extracted> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(40)))
        .http_status_as_error(true)
        .build()
        .new_agent();
    let mut resp = agent
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) marginalia/0.1")
        .header("Accept", "text/html,application/xhtml+xml,application/pdf,text/plain;q=0.8,*/*;q=0.5")
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut bytes = Vec::new();
    resp.body_mut().as_reader().take(64 << 20).read_to_end(&mut bytes)?;
    let fallback_title = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url).to_string();
    if ctype.contains("pdf") || bytes.starts_with(b"%PDF") {
        let pages = pdf_pages_from_mem(&bytes)?;
        return Ok(blocks_from_pages(pages, &fallback_title, DocKind::Pdf));
    }
    let text = String::from_utf8_lossy(&bytes).to_string();
    if ctype.contains("html") || text.trim_start().starts_with('<') {
        let mut e = extract_html(&text, &fallback_title, DocKind::Web);
        if e.title.is_empty() {
            e.title = fallback_title;
        }
        Ok(e)
    } else {
        Ok(extract_plain(&text, &fallback_title, DocKind::Web))
    }
}

// ---------- PDF ----------

fn extract_pdf(path: &Path, stem: &str) -> Result<Extracted> {
    let bytes = std::fs::read(path)?;
    let pages = pdf_pages_from_mem(&bytes)?;
    Ok(blocks_from_pages(pages, stem, DocKind::Pdf))
}

fn pdf_pages_from_mem(bytes: &[u8]) -> Result<Vec<String>> {
    // pdf-extract can panic on exotic files; keep that contained.
    let res = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes));
    match res {
        Ok(Ok(pages)) => Ok(pages),
        Ok(Err(e)) => bail!("PDF extraction failed: {e}"),
        Err(_) => bail!("PDF extraction panicked (unsupported PDF features)"),
    }
}

fn blocks_from_pages(pages: Vec<String>, title: &str, kind: DocKind) -> Extracted {
    let mut blocks = Vec::new();
    let mut doc_title: Option<String> = None;
    for (i, page) in pages.iter().enumerate() {
        blocks.push(Block::Page(i as u32 + 1));
        for para in split_paragraphs(page) {
            if looks_like_heading(&para) {
                if doc_title.is_none() && i == 0 {
                    doc_title = Some(para.clone());
                }
                blocks.push(Block::Heading { level: 2, text: para });
            } else {
                blocks.push(Block::Para(para));
            }
        }
    }
    let title = doc_title.filter(|t| t.len() <= 120).unwrap_or_else(|| title.to_string());
    Extracted { title, kind, blocks }
}

// ---------- Office (zip + xml) ----------

fn read_zip_entry(path: &Path, name: &str) -> Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid Office (zip) file")?;
    let mut entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    let mut s = String::new();
    entry.read_to_string(&mut s)?;
    Ok(Some(s))
}

fn zip_names(path: &Path) -> Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    let zip = zip::ZipArchive::new(file).context("not a valid Office (zip) file")?;
    Ok(zip.file_names().filter_map(|n| n.ok()).map(|s| s.to_string()).collect())
}

/// Walk Office XML and yield (paragraph text, style name) per paragraph element.
fn office_paragraphs(xml: &str, para_tag: &str, text_tag: &str, style_tag: Option<&str>) -> Vec<(String, String)> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let mut in_para = false;
    let mut in_text = false;
    let mut cur = String::new();
    let mut style = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_string();
                if name == para_tag {
                    in_para = true;
                    cur.clear();
                    style.clear();
                } else if name == text_tag {
                    in_text = true;
                } else if name == "w:tab" {
                    cur.push('\t');
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.name().as_ref().to_string();
                if let Some(st) = style_tag {
                    if name == st {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "w:val" {
                                style = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.to_string()).unwrap_or_default();
                            }
                        }
                    }
                }
                if name == "w:br" || name == "a:br" {
                    cur.push('\n');
                }
                if name == "w:tab" {
                    cur.push('\t');
                }
            }
            Ok(Event::Text(t)) => {
                if in_para && in_text {
                    cur.push_str(&text_of(&t));
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_string();
                if name == text_tag {
                    in_text = false;
                } else if name == para_tag && in_para {
                    in_para = false;
                    let t = cur.trim().to_string();
                    if !t.is_empty() {
                        out.push((t, style.clone()));
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

fn text_of(t: &quick_xml::events::BytesText<'_>) -> String {
    let raw = t.xml10_content();
    quick_xml::escape::unescape(&raw).map(|c| c.to_string()).unwrap_or_else(|_| raw.to_string())
}

fn extract_docx(path: &Path, stem: &str) -> Result<Extracted> {
    let xml = read_zip_entry(path, "word/document.xml")?.ok_or_else(|| anyhow!("word/document.xml missing"))?;
    let paras = office_paragraphs(&xml, "w:p", "w:t", Some("w:pStyle"));
    let mut blocks = Vec::new();
    let mut title = None;
    for (text, style) in paras {
        let s = style.to_ascii_lowercase();
        if s == "title" && title.is_none() {
            title = Some(text.clone());
            blocks.push(Block::Heading { level: 1, text });
        } else if let Some(level) = s.strip_prefix("heading").and_then(|n| n.parse::<u8>().ok()) {
            blocks.push(Block::Heading { level: level.clamp(1, 6), text });
        } else {
            blocks.push(Block::Para(text));
        }
    }
    Ok(Extracted { title: title.unwrap_or_else(|| stem.to_string()), kind: DocKind::Word, blocks })
}

fn extract_pptx(path: &Path, stem: &str) -> Result<Extracted> {
    let mut slides: Vec<(u32, String)> = zip_names(path)?
        .into_iter()
        .filter_map(|n| {
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse::<u32>().ok()?;
            Some((num, n))
        })
        .collect();
    slides.sort();
    let mut blocks = Vec::new();
    for (num, name) in slides {
        let Some(xml) = read_zip_entry(path, &name)? else { continue };
        blocks.push(Block::Page(num));
        let paras = office_paragraphs(&xml, "a:p", "a:t", None);
        for (i, (text, _)) in paras.into_iter().enumerate() {
            if i == 0 && text.split_whitespace().count() <= 14 {
                blocks.push(Block::Heading { level: 2, text });
            } else {
                blocks.push(Block::Para(text));
            }
        }
    }
    Ok(Extracted { title: stem.to_string(), kind: DocKind::Slides, blocks })
}

fn extract_xlsx(path: &Path, stem: &str) -> Result<Extracted> {
    use quick_xml::events::Event;
    // Shared strings.
    let mut shared: Vec<String> = Vec::new();
    if let Some(xml) = read_zip_entry(path, "xl/sharedStrings.xml")? {
        let mut reader = quick_xml::Reader::from_str(&xml);
            let mut cur = String::new();
        let mut in_si = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) if e.name().as_ref() == "si" => {
                    in_si = true;
                    cur.clear();
                }
                Ok(Event::Text(t)) if in_si => cur.push_str(&text_of(&t)),
                Ok(Event::End(e)) if e.name().as_ref() == "si" => {
                    in_si = false;
                    shared.push(cur.clone());
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            }
    }
    let mut sheets: Vec<(u32, String)> = zip_names(path)?
        .into_iter()
        .filter_map(|n| {
            let num = n.strip_prefix("xl/worksheets/sheet")?.strip_suffix(".xml")?.parse::<u32>().ok()?;
            Some((num, n))
        })
        .collect();
    sheets.sort();
    let mut blocks = Vec::new();
    for (num, name) in sheets {
        let Some(xml) = read_zip_entry(path, &name)? else { continue };
        blocks.push(Block::Page(num));
        blocks.push(Block::Heading { level: 2, text: format!("Sheet {num}") });
        let mut reader = quick_xml::Reader::from_str(&xml);
            let mut row: Vec<String> = Vec::new();
        let mut cell_type = String::new();
        let mut in_v = false;
        let mut rows_text = Vec::new();
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => match e.name().as_ref() {
                    "row" => row.clear(),
                    "c" => {
                        cell_type.clear();
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "t" {
                                cell_type = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.to_string()).unwrap_or_default();
                            }
                        }
                    }
                    "v" => in_v = true,
                    _ => {}
                },
                Ok(Event::Text(t)) if in_v => {
                    let raw = text_of(&t);
                    let val = if cell_type == "s" {
                        raw.parse::<usize>().ok().and_then(|i| shared.get(i).cloned()).unwrap_or(raw)
                    } else {
                        raw
                    };
                    row.push(val);
                }
                Ok(Event::End(e)) => match e.name().as_ref() {
                    "v" => in_v = false,
                    "row" => {
                        if row.iter().any(|c| !c.trim().is_empty()) {
                            rows_text.push(row.join(" | "));
                        }
                        if rows_text.len() >= 12 {
                            blocks.push(Block::Para(rows_text.join("\n")));
                            rows_text.clear();
                        }
                    }
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
            }
        if !rows_text.is_empty() {
            blocks.push(Block::Para(rows_text.join("\n")));
        }
    }
    Ok(Extracted { title: stem.to_string(), kind: DocKind::Sheet, blocks })
}

// ---------- Markdown / text / HTML ----------

pub fn extract_markdown(text: &str, stem: &str) -> Extracted {
    let mut blocks = Vec::new();
    let mut title: Option<String> = None;
    let mut para = String::new();
    let mut in_code = false;
    let flush = |para: &mut String, blocks: &mut Vec<Block>| {
        let t = para.trim();
        if !t.is_empty() {
            blocks.push(Block::Para(t.to_string()));
        }
        para.clear();
    };
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            in_code = !in_code;
            para.push_str(trimmed);
            para.push('\n');
            continue;
        }
        if in_code {
            para.push_str(line);
            para.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let level = 1 + rest.chars().take_while(|c| *c == '#').count() as u8;
            let heading = rest.trim_start_matches('#').trim().to_string();
            if !heading.is_empty() {
                flush(&mut para, &mut blocks);
                if title.is_none() && level == 1 {
                    title = Some(heading.clone());
                }
                blocks.push(Block::Heading { level: level.min(6), text: heading });
                continue;
            }
        }
        if trimmed.trim().is_empty() {
            flush(&mut para, &mut blocks);
        } else {
            para.push_str(trimmed);
            para.push('\n');
        }
    }
    flush(&mut para, &mut blocks);
    Extracted { title: title.unwrap_or_else(|| stem.to_string()), kind: DocKind::Markdown, blocks }
}

pub fn extract_plain(text: &str, stem: &str, kind: DocKind) -> Extracted {
    let blocks = split_paragraphs(text).into_iter().map(Block::Para).collect();
    Extracted { title: stem.to_string(), kind, blocks }
}

fn extract_table_text(text: &str, stem: &str) -> Extracted {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    for chunk in lines.chunks(15) {
        let t = chunk.join("\n");
        if !t.trim().is_empty() {
            blocks.push(Block::Para(t));
        }
    }
    Extracted { title: stem.to_string(), kind: DocKind::Sheet, blocks }
}

pub fn extract_html(html: &str, stem: &str, kind: DocKind) -> Extracted {
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
        .ok()
        .and_then(|re| re.captures(html))
        .map(|c| decode_entities(&strip_tags(&c[1])).trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| stem.to_string());
    // Remove non-content containers before rendering.
    let cleaned = regex::Regex::new(r"(?is)<(script|style|nav|footer|header|noscript|svg|form|aside)[^>]*>.*?</\1>")
        .map(|re| re.replace_all(html, " ").to_string())
        .unwrap_or_else(|_| html.to_string());
    // Mark headings so they survive text rendering.
    let marked = regex::Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h[1-6]>")
        .map(|re| {
            re.replace_all(&cleaned, |c: &regex::Captures| {
                let level: usize = c[1].parse().unwrap_or(2);
                format!("<p>\u{1}{} {}</p>", "#".repeat(level), strip_tags(&c[2]))
            })
            .to_string()
        })
        .unwrap_or(cleaned);
    let text = html2text::config::plain_no_decorate()
        .no_link_wrapping()
        .string_from_read(marked.as_bytes(), 10_000)
        .unwrap_or_else(|_| strip_tags(&marked));
    let mut blocks = Vec::new();
    for para in split_paragraphs(&text) {
        if let Some(rest) = para.strip_prefix('\u{1}') {
            let level = rest.chars().take_while(|c| *c == '#').count() as u8;
            let heading = rest.trim_start_matches('#').trim().to_string();
            if !heading.is_empty() {
                blocks.push(Block::Heading { level: level.clamp(1, 6), text: heading });
            }
        } else {
            blocks.push(Block::Para(para));
        }
    }
    Extracted { title, kind, blocks }
}

fn strip_tags(s: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    decode_entities(&re.replace_all(s, " "))
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

// ---------- OpenDocument / EPUB / RTF / subtitles ----------

fn extract_opendocument(path: &Path, stem: &str, ext: &str) -> Result<Extracted> {
    let xml = read_zip_entry(path, "content.xml")?.ok_or_else(|| anyhow!("content.xml missing"))?;
    let kind = match ext {
        "odp" => DocKind::Slides,
        "ods" => DocKind::Sheet,
        _ => DocKind::Word,
    };
    // Headings are <text:h text:outline-level="n">, paragraphs <text:p>, cells <table:table-cell>.
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut blocks = Vec::new();
    let mut cur = String::new();
    let mut depth_text = 0i32;
    let mut heading_level: Option<u8> = None;
    let mut page = 0u32;
    let mut row: Vec<String> = Vec::new();
    let mut in_cell = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "text:h" => {
                        depth_text += 1;
                        cur.clear();
                        let mut level = 1u8;
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "text:outline-level" {
                                level = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().and_then(|v| v.parse().ok()).unwrap_or(1);
                            }
                        }
                        heading_level = Some(level.clamp(1, 6));
                    }
                    "text:p" => {
                        depth_text += 1;
                        if depth_text == 1 {
                            cur.clear();
                        }
                    }
                    "draw:page" => {
                        page += 1;
                        blocks.push(Block::Page(page));
                    }
                    "table:table-row" => row.clear(),
                    "table:table-cell" => {
                        in_cell = true;
                        cur.clear();
                    }
                    "text:s" => cur.push(' '),
                    "text:tab" => cur.push('\t'),
                    "text:line-break" => cur.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "text:s" => cur.push(' '),
                "text:tab" => cur.push('\t'),
                "text:line-break" => cur.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if depth_text > 0 || in_cell {
                    cur.push_str(&text_of(&t));
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "text:h" => {
                        depth_text -= 1;
                        let t = cur.trim().to_string();
                        if !t.is_empty() {
                            blocks.push(Block::Heading { level: heading_level.unwrap_or(1), text: t });
                        }
                        cur.clear();
                    }
                    "text:p" => {
                        depth_text -= 1;
                        if depth_text == 0 && !in_cell {
                            let t = cur.trim().to_string();
                            if !t.is_empty() {
                                blocks.push(Block::Para(t));
                            }
                            cur.clear();
                        } else if in_cell {
                            cur.push(' ');
                        }
                    }
                    "table:table-cell" => {
                        in_cell = false;
                        row.push(cur.trim().to_string());
                        cur.clear();
                    }
                    "table:table-row" => {
                        if row.iter().any(|c| !c.is_empty()) {
                            blocks.push(Block::Para(row.join(" | ")));
                        }
                        row.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // Collapse consecutive sheet rows into table paragraphs.
    if kind == DocKind::Sheet {
        let mut merged = Vec::new();
        let mut buf: Vec<String> = Vec::new();
        for b in blocks {
            match b {
                Block::Para(p) if p.contains(" | ") => {
                    buf.push(p);
                    if buf.len() >= 12 {
                        merged.push(Block::Para(buf.join("\n")));
                        buf.clear();
                    }
                }
                other => {
                    if !buf.is_empty() {
                        merged.push(Block::Para(buf.join("\n")));
                        buf.clear();
                    }
                    merged.push(other);
                }
            }
        }
        if !buf.is_empty() {
            merged.push(Block::Para(buf.join("\n")));
        }
        blocks = merged;
    }
    let title = blocks
        .iter()
        .find_map(|b| match b {
            Block::Heading { level: 1, text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| stem.to_string());
    Ok(Extracted { title, kind, blocks })
}

fn extract_epub(path: &Path, stem: &str) -> Result<Extracted> {
    // Reading order comes from the OPF spine; fall back to sorted (x)html entries.
    let names = zip_names(path)?;
    let opf_name = names.iter().find(|n| n.ends_with(".opf")).cloned();
    let mut ordered: Vec<String> = Vec::new();
    let mut title = stem.to_string();
    if let Some(opf) = opf_name.as_ref().and_then(|n| read_zip_entry(path, n).ok().flatten()) {
        if let Some(t) = regex::Regex::new(r"(?is)<dc:title[^>]*>(.*?)</dc:title>").ok().and_then(|re| re.captures(&opf)).map(|c| decode_entities(&c[1]).trim().to_string()) {
            if !t.is_empty() {
                title = t;
            }
        }
        let base = opf_name.as_ref().and_then(|n| n.rfind('/')).map(|i| opf_name.as_ref().unwrap()[..=i].to_string()).unwrap_or_default();
        let mut manifest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if let Ok(re) = regex::Regex::new(r#"(?is)<item\b[^>]*?\bid="([^"]+)"[^>]*?\bhref="([^"]+)""#) {
            for c in re.captures_iter(&opf) {
                manifest.insert(c[1].to_string(), format!("{base}{}", decode_entities(&c[2])));
            }
        }
        if let Ok(re) = regex::Regex::new(r#"(?is)<item\b[^>]*?\bhref="([^"]+)"[^>]*?\bid="([^"]+)""#) {
            for c in re.captures_iter(&opf) {
                manifest.entry(c[2].to_string()).or_insert(format!("{base}{}", decode_entities(&c[1])));
            }
        }
        if let Ok(re) = regex::Regex::new(r#"(?is)<itemref\b[^>]*?\bidref="([^"]+)""#) {
            for c in re.captures_iter(&opf) {
                if let Some(href) = manifest.get(&c[1]) {
                    ordered.push(href.clone());
                }
            }
        }
    }
    if ordered.is_empty() {
        ordered = names.iter().filter(|n| n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm")).cloned().collect();
        ordered.sort();
    }
    let mut blocks = Vec::new();
    for (i, name) in ordered.iter().enumerate() {
        let Some(html) = read_zip_entry(path, name)? else { continue };
        let part = extract_html(&html, "", DocKind::Ebook);
        blocks.push(Block::Page(i as u32 + 1));
        blocks.extend(part.blocks);
    }
    if blocks.is_empty() {
        bail!("no readable chapters in EPUB");
    }
    Ok(Extracted { title, kind: DocKind::Ebook, blocks })
}

/// Minimal RTF to text: strips control words and groups, keeps paragraph breaks.
pub fn extract_rtf(rtf: &str, stem: &str) -> Extracted {
    let mut out = String::new();
    let mut chars = rtf.chars().peekable();
    let mut skip_depth = 0i32;
    let mut depth = 0i32;
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                depth += 1;
                if skip_depth > 0 {
                    skip_depth += 1;
                }
            }
            '}' => {
                depth -= 1;
                if skip_depth > 0 {
                    skip_depth -= 1;
                }
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    // Escaped char or special.
                    if let Some(n) = chars.next() {
                        match n {
                            '\'' => {
                                let hex: String = chars.by_ref().take(2).collect();
                                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                                    if skip_depth == 0 {
                                        out.push(b as char);
                                    }
                                }
                            }
                            '*' => skip_depth = 1,
                            '\\' | '{' | '}' => {
                                if skip_depth == 0 {
                                    out.push(n);
                                }
                            }
                            '\n' | '\r' => {
                                if skip_depth == 0 {
                                    out.push('\n');
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                // numeric parameter
                let mut param = String::new();
                while let Some(&n) = chars.peek() {
                    if n == '-' || n.is_ascii_digit() {
                        param.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                match word.as_str() {
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header" | "footer" | "field" | "fldinst" | "themedata" | "colorschememapping" | "latentstyles" | "datastore" => {
                        if skip_depth == 0 {
                            skip_depth = 1;
                        }
                    }
                    "par" | "line" | "sect" | "page" => {
                        if skip_depth == 0 {
                            out.push('\n');
                        }
                    }
                    "tab" => {
                        if skip_depth == 0 {
                            out.push('\t');
                        }
                    }
                    "u" => {
                        if let Ok(code) = param.parse::<i32>() {
                            let code = if code < 0 { code + 65536 } else { code };
                            if let Some(ch) = char::from_u32(code as u32) {
                                if skip_depth == 0 {
                                    out.push(ch);
                                }
                            }
                        }
                        // skip the fallback character
                        chars.next();
                    }
                    _ => {}
                }
            }
            '\n' | '\r' => {}
            _ => {
                if skip_depth == 0 && depth > 0 {
                    out.push(c);
                }
            }
        }
    }
    extract_plain(&out, stem, DocKind::Word)
}

/// SRT / WebVTT subtitles → timestamped paragraphs.
fn extract_subtitles(text: &str, stem: &str) -> Extracted {
    let ts = regex::Regex::new(r"(\d{1,2}:\d{2}(?::\d{2})?[.,]\d{1,3})\s*-->").unwrap();
    let mut blocks = Vec::new();
    let mut cur = String::new();
    let mut cur_start: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.chars().all(|c| c.is_ascii_digit()) || l.starts_with("WEBVTT") {
            continue;
        }
        if let Some(c) = ts.captures(l) {
            if cur_start.is_none() {
                cur_start = Some(c[1].to_string());
            }
            continue;
        }
        cur.push_str(l);
        cur.push(' ');
        if cur.len() > 700 {
            let start = cur_start.take().unwrap_or_default();
            blocks.push(Block::Heading { level: 3, text: format!("[{start}]") });
            blocks.push(Block::Para(cur.trim().to_string()));
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        let start = cur_start.take().unwrap_or_default();
        blocks.push(Block::Heading { level: 3, text: format!("[{start}]") });
        blocks.push(Block::Para(cur.trim().to_string()));
    }
    Extracted { title: stem.to_string(), kind: DocKind::Text, blocks }
}

// ---------- helpers ----------

pub fn split_paragraphs(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\u{c}', "\n\n");
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in normalized.lines() {
        let l = line.trim();
        if l.is_empty() {
            if !cur.trim().is_empty() {
                out.push(normalize_ws(&cur));
            }
            cur.clear();
        } else {
            if !cur.is_empty() {
                // Re-join hyphenated line breaks from PDFs.
                if cur.ends_with('-') && l.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
                    cur.pop();
                } else {
                    cur.push(' ');
                }
            }
            cur.push_str(l);
        }
    }
    if !cur.trim().is_empty() {
        out.push(normalize_ws(&cur));
    }
    out
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_heading(p: &str) -> bool {
    let words: Vec<&str> = p.split_whitespace().collect();
    if words.is_empty() || words.len() > 12 || p.len() > 90 {
        return false;
    }
    if p.ends_with('.') || p.ends_with(',') || p.ends_with(';') {
        return false;
    }
    let numbered = regex::Regex::new(r"^(\d+(\.\d+)*\.?|[IVX]+\.|[A-Z]\.)\s+\S").unwrap().is_match(p);
    let caps = words.iter().filter(|w| w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)).count();
    let all_caps = p.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) && p.len() > 3;
    numbered || all_caps || (caps * 10 >= words.len() * 7 && words.len() <= 8)
}
