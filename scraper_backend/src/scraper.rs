use super::{Site, UpdateMessage, AppConfig};
use chrono::{Utc, DateTime};
use rand::{seq::SliceRandom, thread_rng, Rng};
use reqwest::header::{HeaderMap, USER_AGENT};
use sha2::{Sha256, Digest};
use sqlx::{Pool, Sqlite};
use tokio::{time::{sleep, Duration}, sync::broadcast::Sender};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// HTML tag and processing dependencies
use regex::Regex;
use scraper::{Html, Selector};

// Track site check intervals and backoff state
type SiteState = Arc<RwLock<HashMap<i64, SiteCheckState>>>;

#[derive(Debug, Clone)]
struct SiteCheckState {
    next_check: DateTime<Utc>,
    backoff_count: u32,
}

pub async fn run_scraper(pool: Pool<Sqlite>, tx: Sender<UpdateMessage>, config: AppConfig) {
    println!("---------------------------------------------");
    println!("Scraper background task started successfully");
    println!("Will check for site updates in the background");
    
    // Create shared state for tracking site check schedules
    let site_states: SiteState = Arc::new(RwLock::new(HashMap::new()));
    
    // Convert config to Arc to share across tasks
    let config = Arc::new(config);
    
    loop {
        let sites: Vec<Site> = sqlx::query_as::<_, Site>("SELECT * FROM sites")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

        let now = Utc::now();
        
        for site in sites {
            let site_id = site.id;
            let should_check = {
                let states = site_states.read().await;
                if let Some(state) = states.get(&site_id) {
                    now >= state.next_check
                } else {
                    true // First time seeing this site, check it now
                }
            };
            
            if should_check {
                // spawn per site
                let pool_clone = pool.clone();
                let tx_clone = tx.clone();
                let site_states_clone = site_states.clone();
                let config_clone = config.clone();
                
                tokio::spawn(async move {
                    check_site(site, pool_clone, tx_clone, site_states_clone, &config_clone).await;
                });
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn check_site(site: Site, pool: Pool<Sqlite>, tx: Sender<UpdateMessage>, site_states: SiteState, config: &Arc<AppConfig>) {
    let mut headers = HeaderMap::new();
    let agents = vec![
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        "Mozilla/5.0 (X11; Linux x86_64)",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 14_0 like Mac OS X)",
    ];
    headers.insert(USER_AGENT, agents.choose(&mut thread_rng()).unwrap().parse().unwrap());

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // fetch
    let body_res = client.get(&site.url).send().await;
    let fetched_at = Utc::now();
    let mut success = true;
    
    if let Ok(resp) = body_res {
        if let Ok(body) = resp.text().await {
            // Pre-process content to remove volatile elements before hashing
            let cleaned_content = clean_content_for_comparison(&body);
            
            // Hash the cleaned content
            let mut hasher = Sha256::new();
            hasher.update(cleaned_content.as_bytes());
            let hash = format!("{:x}", hasher.finalize());

            let last_hash: Option<(String,)> = sqlx::query_as("SELECT diff_hash FROM updates WHERE site_id = ?1 ORDER BY id DESC LIMIT 1")
                .bind(site.id)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();

            let changed = last_hash.map_or(true, |h| h.0 != hash);

            // Update last_checked
            sqlx::query!("UPDATE sites SET last_checked = ?1, status = 'OK' WHERE id = ?2", fetched_at, site.id)
                .execute(&pool)
                .await
                .unwrap();

            // Store every fetch in the database regardless of change
            sqlx::query!("INSERT INTO updates(site_id, timestamp, diff_hash, content) VALUES (?1, ?2, ?3, ?4)",
                site.id, fetched_at, hash, body)
                .execute(&pool)
                .await
                .unwrap();

            // Only notify UI if content meaningfully changed
            if changed {
                // Extract and format a better content preview
                let content_preview = extract_formatted_preview(&body, 400);
                
                // Notify about the update
                let _ = tx.send(UpdateMessage{
                    site_id: site.id,
                    url: site.url.clone(),
                    timestamp: fetched_at,
                    diff_hash: hash,
                    content_preview,
                    has_full_content: true,
                });
                
                // Update last_updated timestamp
                sqlx::query!("UPDATE sites SET last_updated = ?1 WHERE id = ?2", fetched_at, site.id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            
            // Limit the number of updates stored per site based on config
            let update_cache_size = config.update_cache_size;
            sqlx::query!(
                "DELETE FROM updates WHERE id IN (
                    SELECT id FROM updates 
                    WHERE site_id = ?1 
                    ORDER BY id DESC 
                    LIMIT -1 OFFSET ?2
                )",
                site.id,
                update_cache_size
            )
            .execute(&pool)
            .await
            .unwrap();
        } else {
            success = false;
        }
    } else {
        success = false;
        sqlx::query!("UPDATE sites SET last_checked = ?1, status = 'ERROR' WHERE id = ?2",
            fetched_at, site.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    
    // Calculate next check time based on style and interval
    let mut backoff_count = 0;
    
    // Get current state if it exists
    if let Some(current_state) = site_states.read().await.get(&site.id) {
        backoff_count = current_state.backoff_count;
    }
    
    // Determine next check time based on style
    let next_check_time = match site.style.as_str() {
        "random" => {
            // Add the configured interval plus a random jitter
            let jitter_ms = thread_rng().gen_range(0..config.interval_jitter_max_ms as u64);
            let interval_ms = site.interval_secs * 1000 + jitter_ms as i64;
            fetched_at + chrono::Duration::milliseconds(interval_ms)
        },
        "exponential" => {
            if success {
                // Reset backoff on success
                backoff_count = 0;
                fetched_at + chrono::Duration::seconds(site.interval_secs)
            } else {
                // Double wait time on failure, up to a reasonable maximum
                backoff_count += 1;
                let backoff_interval = site.interval_secs * 2i64.pow(backoff_count.min(10)); // Cap at 10 to avoid overflow
                fetched_at + chrono::Duration::seconds(backoff_interval)
            }
        },
        _ => {
            // "none" style or any unrecognized style - fixed interval only
            fetched_at + chrono::Duration::seconds(site.interval_secs)
        }
    };
    
    // Update the site state
    let mut states = site_states.write().await;
    states.insert(site.id, SiteCheckState {
        next_check: next_check_time,
        backoff_count,
    });
}

// Extract and format a preview of the content
fn extract_formatted_preview(content: &str, max_length: usize) -> String {
    // First check if it's RSS or XML content
    if content.contains("<?xml") || content.contains("<rss") || content.contains("<feed") || 
       content.contains("<item>") || content.contains("<entry>") {
        return extract_rss_preview(content, max_length);
    }
    
    // Special handler for Reddit content
    if content.contains("/u/DeepFuckingValue") || content.contains("r/wallstreetbets") || 
       content.contains("reddit.com") {
        return extract_reddit_preview(content, max_length);
    }
    
    // Check if it's JSON content
    if (content.starts_with('{') && content.ends_with('}')) || 
       (content.starts_with('[') && content.ends_with(']')) {
        return extract_json_preview(content, max_length);
    }
    
    // Check for JavaScript code
    if content.contains("function(") || content.contains("var ") || content.contains("const ") {
        // This is likely JavaScript or has a lot of JavaScript, clean it first
        let cleaned = clean_script_content(content);
        return format!("📰 Content Preview\n\n{}", if cleaned.len() > max_length {
            format!("{}...", &cleaned[..find_word_boundary(&cleaned, max_length)])
        } else {
            cleaned
        });
    }

    // Process HTML content using a more robust approach
    extract_html_preview(content, max_length)
}

// Extract preview from HTML content using the HTML parser
fn extract_html_preview(html: &str, max_length: usize) -> String {
    // Create a new HTML document for parsing
    let document = Html::parse_document(html);
    
    // Try to find the title
    let mut title = String::new();
    
    // First check for title tag
    if let Ok(title_selector) = Selector::parse("title") {
        if let Some(title_element) = document.select(&title_selector).next() {
            title = title_element.text().collect::<Vec<_>>().join(" ").trim().to_string();
        }
    }
    
    // If no title, try h1
    if title.is_empty() {
        if let Ok(h1_selector) = Selector::parse("h1") {
            if let Some(h1_element) = document.select(&h1_selector).next() {
                title = h1_element.text().collect::<Vec<_>>().join(" ").trim().to_string();
            }
        }
    }
    
    // Start building the preview
    let mut preview = String::new();
    
    // Add the title with formatting if found
    if !title.is_empty() {
        preview.push_str(&format!("📰 {}\n\n", title));
    }
    
    // Try to extract meaningful content
    let mut content_text = String::new();
    
    // Try various content selectors by priority
    let content_selectors = [
        "article", "main", ".content", "#content", ".post-content", 
        ".entry-content", ".article-content", ".post", "p",
        ".news-article", ".article__content", ".story-body", ".story__content"
    ];
    
    for selector_str in content_selectors {
        if let Ok(selector) = Selector::parse(selector_str) {
            let elements: Vec<_> = document.select(&selector).collect();
            if !elements.is_empty() {
                // Join text from all matching elements
                content_text = elements.iter()
                    .flat_map(|el| el.text())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                
                // Remove excessive whitespace
                content_text = normalize_whitespace(&content_text);
                
                if !content_text.is_empty() {
                    break;
                }
            }
        }
    }
    
    // If we couldn't extract content with selectors, fall back to general text extraction
    if content_text.is_empty() {
        // Extract all text from body
        if let Ok(body_selector) = Selector::parse("body") {
            if let Some(body) = document.select(&body_selector).next() {
                content_text = body.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                
                content_text = normalize_whitespace(&content_text);
                content_text = clean_script_content(&content_text);
            }
        }
    }
    
    // Add content preview with length limit
    if !content_text.is_empty() {
        let content_preview = if content_text.len() > max_length {
            // Try to cut at a sentence boundary if possible
            let cutoff = find_sentence_boundary(&content_text, max_length);
            format!("{}...", &content_text[..cutoff])
        } else {
            content_text
        };
        
        preview.push_str(&content_preview);
    } else if !preview.is_empty() {
        // If we only have a title, add a placeholder for content
        preview.push_str("[Content not available]");
    } else {
        // Complete fallback if we couldn't extract anything meaningful
        preview = "Unable to extract readable content from this page.".to_string();
    }
    
    preview
}

// Extract preview from RSS/XML content
fn extract_rss_preview(xml: &str, max_length: usize) -> String {
    let mut preview = String::new();
    
    // Very basic XML tag extraction
    // Look for common RSS/feed elements
    let title_pattern = Regex::new(r"<title[^>]*>(.*?)</title>").unwrap_or_else(|_| Regex::new(r"").unwrap());
    let desc_pattern = Regex::new(r"<description[^>]*>(.*?)</description>").unwrap_or_else(|_| Regex::new(r"").unwrap());
    let content_pattern = Regex::new(r"<content[^>]*>(.*?)</content>").unwrap_or_else(|_| Regex::new(r"").unwrap());
    
    // Extract title 
    if let Some(captures) = title_pattern.captures(xml) {
        if let Some(title_match) = captures.get(1) {
            let title = clean_xml_entities(title_match.as_str());
            if !title.is_empty() {
                preview.push_str(&format!("📰 {}\n\n", title));
            }
        }
    }
    
    // Try to extract content (prioritize content over description)
    let mut content_text = String::new();
    
    if let Some(captures) = content_pattern.captures(xml) {
        if let Some(content_match) = captures.get(1) {
            content_text = clean_xml_entities(content_match.as_str());
        }
    }
    
    // If no content, try description
    if content_text.is_empty() {
        if let Some(captures) = desc_pattern.captures(xml) {
            if let Some(desc_match) = captures.get(1) {
                content_text = clean_xml_entities(desc_match.as_str());
            }
        }
    }
    
    // If still no content, try extracting from CDATA sections
    if content_text.is_empty() {
        let cdata_pattern = Regex::new(r"<!\[CDATA\[(.*?)\]\]>").unwrap_or_else(|_| Regex::new(r"").unwrap());
        if let Some(captures) = cdata_pattern.captures(xml) {
            if let Some(cdata_match) = captures.get(1) {
                content_text = clean_html_content(cdata_match.as_str());
            }
        }
    }
    
    // Add content with length limit
    if !content_text.is_empty() {
        let cutoff = find_word_boundary(&content_text, max_length);
        let content_preview = if content_text.len() > max_length {
            format!("{}...", &content_text[..cutoff])
        } else {
            content_text
        };
        
        preview.push_str(&content_preview);
    } else if !preview.is_empty() {
        // If we only have a title, add a placeholder
        preview.push_str("[RSS feed detected - content not available]");
    } else {
        // Complete fallback
        preview = "RSS/XML content detected, but couldn't extract readable content.".to_string();
    }
    
    preview
}

// Extract preview from JSON content
fn extract_json_preview(json: &str, max_length: usize) -> String {
    // Simple JSON preview - for now just indicate it's JSON and show a sample
    let mut preview = String::new();
    
    preview.push_str("📊 JSON Data\n\n");
    
    // Get a short sample of the JSON
    let content_preview = if json.len() > max_length {
        format!("{}...", &json[..find_word_boundary(json, max_length)])
    } else {
        json.to_string()
    };
    
    preview.push_str(&content_preview);
    
    preview
}

// Clean HTML content for better readability
fn clean_html_content(html: &str) -> String {
    // Initialize regex only once if performance becomes an issue
    let tag_pattern = Regex::new(r"<[^>]*>").unwrap_or_else(|_| Regex::new(r"").unwrap());
    
    // Replace common HTML entities
    let mut text = clean_xml_entities(html);
    
    // Remove HTML tags
    text = tag_pattern.replace_all(&text, " ").to_string();
    
    // Normalize whitespace
    text = normalize_whitespace(&text);
    
    // Remove common JavaScript patterns that often appear in content
    text = clean_script_content(&text);
    
    text
}

// Clean XML/HTML entities
fn clean_xml_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&ndash;", "-")
        .replace("&mdash;", "-")
        .replace("&lsquo;", "'")
        .replace("&rsquo;", "'")
        .replace("&ldquo;", "\"")
        .replace("&rdquo;", "\"")
}

// Normalize whitespace
fn normalize_whitespace(text: &str) -> String {
    // Replace multiple spaces, tabs, newlines with single space
    let ws_pattern = Regex::new(r"\s+").unwrap_or_else(|_| Regex::new(r" ").unwrap());
    ws_pattern.replace_all(text, " ").to_string()
}

// Clean out JavaScript content that often gets mixed into scraped content
fn clean_script_content(text: &str) -> String {
    // Common patterns found in JavaScript that leak into content
    let js_patterns = [
        r"function\s*\([^)]*\)\s*\{[^}]*\}",
        r"var\s+\w+\s*=",
        r"const\s+\w+\s*=",
        r"let\s+\w+\s*=",
        r"if\s*\([^)]*\)",
        r"window\.\w+",
        r"document\.\w+",
        r"\(\s*function\s*\(\)",
        r"\/\*.*?\*\/",
        r"\/\/.*?[\n\r]",
        r"gtag\([^)]*\)",
        r"dataLayer",
        r"GoogleAnalytics",
        r"google-analytics",
        r"googletag",
    ];
    
    let mut cleaned = text.to_string();
    
    for pattern in js_patterns {
        if let Ok(re) = Regex::new(pattern) {
            cleaned = re.replace_all(&cleaned, " ").to_string();
        }
    }
    
    // Remove sequences that look like script code
    if let Ok(re) = Regex::new(r"\{[^{}]*\{[^{}]*\}[^{}]*\}") {
        cleaned = re.replace_all(&cleaned, " ").to_string();
    }
    
    normalize_whitespace(&cleaned)
}

// Extract preview from Reddit content
fn extract_reddit_preview(content: &str, max_length: usize) -> String {
    // Reddit content often has a specific format we can parse
    let mut preview = String::new();
    
    // Try to find post titles with a simple regex
    let post_pattern = Regex::new(r"GME YOLO [^\n\r]+ r/[^\n\r]+").unwrap_or_else(|_| Regex::new(r"").unwrap());
    if let Some(post_match) = post_pattern.find(content) {
        preview.push_str(&format!("📈 {}\n\n", post_match.as_str()));
    } else {
        preview.push_str("📈 Reddit Updates\n\n");
    }
    
    // Try to extract reddit username and post info
    let username_pattern = Regex::new(r"/u/([A-Za-z0-9_-]+)").unwrap_or_else(|_| Regex::new(r"").unwrap());
    if let Some(captures) = username_pattern.captures(content) {
        if let Some(username) = captures.get(1) {
            preview.push_str(&format!("User: u/{}\n", username.as_str()));
        }
    }
    
    // Extract subreddit if present
    let subreddit_pattern = Regex::new(r"r/([A-Za-z0-9_-]+)").unwrap_or_else(|_| Regex::new(r"").unwrap());
    if let Some(captures) = subreddit_pattern.captures(content) {
        if let Some(subreddit) = captures.get(1) {
            preview.push_str(&format!("Subreddit: r/{}\n\n", subreddit.as_str()));
        }
    }
    
    // Clean the overall content
    let cleaned = content
        .lines()
        .filter(|line| {
            !line.contains("function") && 
            !line.contains("var ") && 
            !line.contains(".js") && 
            !line.trim().is_empty() && 
            line.len() > 5
        })
        .collect::<Vec<_>>()
        .join("\n");
    
    // Add limited content
    if !cleaned.is_empty() {
        let cutoff = find_word_boundary(&cleaned, max_length);
        let content_preview = if cleaned.len() > max_length {
            format!("{}...", &cleaned[..cutoff])
        } else {
            cleaned
        };
        
        preview.push_str(&content_preview);
    }
    
    preview
}

// Find a reasonable word boundary to cut text at
fn find_word_boundary(text: &str, max_length: usize) -> usize {
    if text.len() <= max_length {
        return text.len();
    }
    
    // Try to find a space, period, comma, or other natural break
    let break_chars = [' ', '.', ',', ';', ':', '!', '?', '\n', '\r'];
    
    // Start from max_length and go backwards
    for i in (0..max_length).rev() {
        if i < text.len() {
            let c = text.chars().nth(i).unwrap();
            if break_chars.contains(&c) {
                return i + 1; // Include the break character
            }
        }
    }
    
    // If no good break found, just use the max_length
    max_length
}

// Add this new function to clean content before comparing (for better delta detection)
fn clean_content_for_comparison(content: &str) -> String {
    // Step 1: Remove common dynamic elements
    let mut cleaned = content.to_string();
    
    // Remove timestamps, dates, and common dynamic patterns
    let patterns_to_remove = [
        r"\d{1,2}:\d{2}:\d{2}",                      // Time (HH:MM:SS)
        r"\d{1,2}:\d{2}",                           // Time (HH:MM)
        r"\d{1,2}/\d{1,2}/\d{2,4}",                 // Date (MM/DD/YYYY)
        r"\d{4}-\d{2}-\d{2}",                       // ISO date (YYYY-MM-DD)
        r"[A-Za-z]{3},\s\d{1,2}\s[A-Za-z]{3}\s\d{4}", // Day, DD Mon YYYY
        r"viewcount[\"']?\s*:\s*[\"']?\d+",         // View counts
        r"[\"']timestamp[\"']\s*:\s*\d+",           // Timestamps in JSON
        r"data-timestamp=[\"']\d+[\"']",            // Data timestamps
        r"<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>", // Scripts
        r"<iframe\b[^<]*(?:(?!<\/iframe>)<[^<]*)*<\/iframe>", // iframes
        r"<!--.*?-->",                               // HTML comments
        r"<ins\b[^<]*(?:(?!<\/ins>)<[^<]*)*<\/ins>", // Ad insertions
    ];
    
    // Apply regex replacements
    for pattern in patterns_to_remove {
        if let Ok(re) = regex::Regex::new(pattern) {
            cleaned = re.replace_all(&cleaned, "").to_string();
        }
    }
    
    // Step 2: Optional - extract only the relevant content
    // This depends on the website structure, but we can add a generic implementation
    // For example, focus on main content areas and ignore headers, footers, sidebars
    let content_selectors = [
        r"<article.*?>(.*?)</article>",
        r"<main.*?>(.*?)</main>",
        r"<div.*?class=[\"\']content[\"\'].*?>(.*?)</div>",
        r"<div.*?class=[\"\']post-content[\"\'].*?>(.*?)</div>",
        r"<div.*?id=[\"\']content[\"\'].*?>(.*?)</div>",
    ];
    
    let mut extracted_content = String::new();
    for selector in content_selectors {
        if let Ok(re) = regex::Regex::new(selector) {
            if let Some(caps) = re.captures(&cleaned) {
                if let Some(m) = caps.get(1) {
                    extracted_content = m.as_str().to_string();
                    break;
                }
            }
        }
    }
    
    // If we extracted specific content, use that; otherwise use the whole cleaned content
    if !extracted_content.is_empty() {
        cleaned = extracted_content;
    }
    
    // Step 3: Normalize whitespace
    normalize_whitespace(&cleaned)
}

// Add function to find sentence boundaries for better excerpt cutting
fn find_sentence_boundary(text: &str, max_length: usize) -> usize {
    if text.len() <= max_length {
        return text.len();
    }
    
    // Look for sentence-ending punctuation
    let sentence_breaks = ['.', '!', '?', '\n', '\r'];
    
    // Start from max_length and go backwards
    for i in (0..max_length).rev() {
        if i < text.len() {
            let c = text.chars().nth(i).unwrap();
            if sentence_breaks.contains(&c) {
                return i + 1; // Include the punctuation
            }
        }
    }
    
    // Fall back to word boundary if no sentence break found
    find_word_boundary(text, max_length)
}