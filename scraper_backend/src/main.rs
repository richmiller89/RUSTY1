use actix_web::{web, App, HttpServer, HttpResponse, Responder};
use actix_files::Files;
use actix_web::middleware::Logger;
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, FromRow};
use std::sync::Arc;
use tokio::sync::broadcast;
use chrono::{DateTime, Utc};

mod scraper;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    tx_updates: broadcast::Sender<UpdateMessage>,
    config: AppConfig,
}

#[derive(Clone, Debug)]
struct AppConfig {
    update_cache_size: i64,
    default_interval_secs: i64,
    interval_jitter_max_ms: i64,
}

#[derive(Serialize, Deserialize, FromRow, Clone)]
struct Site {
    id: i64,
    url: String,
    interval_secs: i64,
    style: String,
    last_checked: Option<DateTime<Utc>>,
    last_updated: Option<DateTime<Utc>>,
    status: Option<String>,
}

#[derive(Serialize, Clone)]
struct UpdateMessage {
    site_id: i64,
    url: String,
    timestamp: DateTime<Utc>,
    diff_hash: String,
    content_preview: String,
    has_full_content: bool,
}

#[derive(Deserialize)]
struct NewSite {
    url: String,
    interval_secs: Option<i64>,
    style: Option<String>,
}

async fn list_sites(data: web::Data<AppState>) -> impl Responder {
    let sites: Vec<Site> = sqlx::query_as::<_, Site>("SELECT * FROM sites")
        .fetch_all(&data.pool)
        .await
        .unwrap_or_default();
    HttpResponse::Ok().json(sites)
}

async fn add_site(data: web::Data<AppState>, payload: web::Json<NewSite>) -> impl Responder {
    let interval = payload.interval_secs.unwrap_or(data.config.default_interval_secs);
    let style = payload.style.clone().unwrap_or_else(|| "random".into());

    let rec = sqlx::query!(
        "INSERT INTO sites (url, interval_secs, style) VALUES (?1, ?2, ?3)",
        payload.url,
        interval,
        style
    )
    .execute(&data.pool)
    .await;

    match rec {
        Ok(result) => HttpResponse::Ok().body(format!("Site added, id={:?}", result.last_insert_rowid())),
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

async fn delete_site(data: web::Data<AppState>, path: web::Path<i64>) -> impl Responder {
    let id = path.into_inner();
    
    // Log the deletion attempt for debugging
    println!("Attempting to delete site with ID: {}", id);
    
    // Make sure foreign keys are enabled for this connection
    let _ = sqlx::query("PRAGMA foreign_keys = ON;").execute(&data.pool).await;
    
    // First, manually delete any updates for this site
    println!("Deleting any updates for site {}", id);
    let _ = sqlx::query!("DELETE FROM updates WHERE site_id = ?1", id)
        .execute(&data.pool)
        .await;
    
    // Check if the site exists before trying to delete
    let site_exists = sqlx::query!("SELECT id FROM sites WHERE id = ?1", id)
        .fetch_optional(&data.pool)
        .await;
    
    match site_exists {
        Ok(Some(_)) => {
            // Now delete the site itself
            let result = sqlx::query!("DELETE FROM sites WHERE id = ?1", id)
                .execute(&data.pool)
                .await;
            
            match result {
                Ok(result) => {
                    println!("Successfully deleted site {}, rows affected: {}", id, result.rows_affected());
                    HttpResponse::Ok().finish()
                },
                Err(e) => {
                    println!("Error deleting site {}: {}", id, e);
                    HttpResponse::InternalServerError().body(format!("Database error: {}", e))
                }
            }
        },
        Ok(None) => {
            println!("Site {} not found for deletion", id);
            HttpResponse::NotFound().body(format!("Site with ID {} not found", id))
        },
        Err(e) => {
            println!("Error checking if site {} exists: {}", id, e);
            HttpResponse::InternalServerError().body(format!("Database error: {}", e))
        },
    }
}

async fn sse_updates(data: web::Data<AppState>, _req: actix_web::HttpRequest) -> impl Responder {
    let mut rx = data.tx_updates.subscribe();
    let stream = async_stream::stream! {
        while let Ok(msg) = rx.recv().await {
            let json = serde_json::to_string(&msg).unwrap();
            yield Ok::<_, actix_web::Error>(actix_web::web::Bytes::from(format!("data: {}\n\n", json)));
        }
    };
    HttpResponse::Ok()
        .insert_header(("Content-Type", "text/event-stream"))
        .streaming(stream)
}

// List of default sites to add when the database is initialized
async fn add_default_sites(pool: &SqlitePool) {
    // Define the default sites - this replaces the hardcoded example sites from the frontend
    let default_sites = vec![
        ("https://ag.ny.gov/press-releases", 1100, "random"),
        ("https://apnews.com/article/feed", 1100, "random"),
        ("https://apnews.com/index.rss", 1100, "random"),
        ("https://asia.nikkei.com/rss/feed/nar", 1100, "random"),
        ("https://citronresearch.com/feed/", 1100, "random"),
        ("https://defence-blog.com/feed/", 1100, "random"),
        ("https://endpts.com/feed/", 1100, "random"),
        ("https://feeds.feedburner.com/nvidiablog", 1100, "random"),
        ("https://fuzzypandaresearch.com/feed/", 1100, "random"),
        ("https://grizzlyreports.com/?feed=rss2", 1100, "random"),
        ("https://hindenburgresearch.com/feed/", 1100, "random"),
        ("https://home.treasury.gov/news/press-releases", 1800, "random"),
        ("https://iceberg-research.com/2023/feed/", 1100, "random"),
        ("https://investor.nvidia.com/rss/SECFiling.aspx?Exchange=CIK&Symbol=0001045810", 1800, "random"),
        ("https://investor.regeneron.com/rss/news-releases.xml?items=15", 1800, "random"),
        ("https://investors.arm.com/financials/quarterly-annual-results", 1800, "random"),
        ("https://investors.block.xyz/financials/quarterly-earnings-reports/default.aspx", 1800, "random"),
        ("https://investors.pfizer.com/Investors/News/default.aspx", 1800, "random"),
        ("https://ir.amd.com/news-events/press-releases/rss", 1800, "random"),
        ("https://ir.cytokinetics.com/press-releases", 1800, "random"),
        ("https://ir.netflix.net/financials/quarterly-earnings/default.aspx", 1800, "random"),
        ("https://ir.netflix.net/rss/SECFiling.aspx?Exchange=CIK&Symbol=0001065280", 1800, "random"),
        ("https://ir.purecycle.com/news-events/press-releases/rss", 1800, "random"),
        ("https://ir.supermicro.com/financials/sec-filings/default.aspx", 1800, "random"),
        ("https://ir.supermicro.com/news/default.aspx", 1800, "random"),
        ("https://ir.tesla.com/#quarterly-disclosure/", 1800, "random"),
        ("https://ir.tesla.com/press", 1800, "random"),
        ("https://ir.tmtgcorp.com/financials/sec-filings/", 1800, "random"),
        ("https://listingcenter.nasdaq.com/IssuersPendingSuspensionDelisting.aspx", 1800, "random"),
        ("https://newsroom.thecignagroup.com/latest-press-releases?pagetemplate=rss", 1800, "random"),
        ("https://nvidianews.nvidia.com/cats/press_release.xml", 1100, "random"),
        ("https://punchbowl.news/feed/", 1100, "random"),
        ("https://scorpioncapital.com/", 1100, "random"),
        ("https://search.cnbc.com/rs/search/combinedcms/view.xml?partnerId=wrss01&id=100003114", 1100, "random"),
        ("https://techcrunch.com/feed/", 1100, "random"),
        ("https://theaircurrent.com/author/jonostrower/feed/", 1100, "random"),
        ("https://thebearcave.substack.com/feed", 1100, "random"),
        ("https://truthsocial.com/@realDonaldTrump", 1100, "random"),
        ("https://www.accessdata.fda.gov/scripts/cder/daf/index.cfm?event=report.page", 1800, "random"),
        ("https://www.accessdata.fda.gov/scripts/cder/daf/index.cfm?event=reportsSearch.process", 1800, "random"),
        ("https://www.accessdata.fda.gov/scripts/drugshortages/dsp_ActiveIngredientDetails.cfm?AI=Semaglutide%20Injection&st=c&tab=tabs-1", 1800, "random"),
        ("https://www.axios.com/feeds/feed.rss", 1100, "random"),
        ("https://www.axios.com/pro", 1100, "random"),
        ("https://www.axios.com/pro/energy-policy/2025/05", 1100, "random"),
        ("https://www.betaville.co.uk/", 1100, "random"),
        ("https://www.biopharmadive.com/feeds/news/", 1100, "random"),
        ("https://www.cadc.uscourts.gov/internet/judgments.nsf/uscadcjudgments.xml", 1800, "random"),
        ("https://www.ded.uscourts.gov/judges-info/opinions?field_opinion_date_value%5Bvalue%5D%5Byear%5D=2024&field_judge_nid=All", 1800, "random"),
        ("https://www.digitimes.com/rss/daily.xml", 1100, "random"),
        ("https://www.dtcc.com/products/cs/exchange_traded_funds_plain_new.php", 1800, "random"),
        ("https://www.fda.gov/about-fda/contact-fda/stay-informed/rss-feeds/medwatch/rss.xml", 1800, "random"),
        ("https://www.fda.gov/about-fda/contact-fda/stay-informed/rss-feeds/oci-press-releases/rss.xml", 1800, "random"),
        ("https://www.fda.gov/about-fda/contact-fda/stay-informed/rss-feeds/press-releases/rss.xml", 1800, "random"),
        ("https://www.federalregister.gov/api/v1/documents.rss", 1800, "random"),
        ("https://www.ft.com/myft/following/b013133b-aba9-4ba5-8f97-e3b6c46f6665.rss", 1100, "random"),
        ("https://www.ftc.gov/feeds/press-release.xml", 1800, "random"),
        ("https://www.gothamcityresearch.com/main/", 1100, "random"),
        ("https://www.jcapitalresearch.com/company-reports.html", 1100, "random"),
        ("https://www.mofcom.gov.cn/xwfb/xwfyrth/index.html", 1800, "random"),
        ("https://www.morpheus-research.com/rss/", 1100, "random"),
        ("https://www.politico.com/rss/politicopicks.xml", 1100, "random"),
        ("https://www.reddit.com/user/AVOCADO-IN-MY-ANUS/.rss", 1100, "random"),
        ("https://www.reddit.com/user/DeepFuckingValue/comments.rss", 1100, "random"),
        ("https://www.reddit.com/user/DeepFuckingValue/submitted.rss", 1100, "random"),
        ("https://www.rockstargames.com/newswire", 1800, "random"),
        ("https://www.sec.gov/Archives/edgar/usgaap.rss.xml", 1800, "random"),
        ("https://www.sec.gov/news/pressreleases.rss", 1800, "random"),
        ("https://www.sec.gov/rules/sro/national-securities-exchanges?aId=&sro_organization=All&title=&release_number=&file_number=&year=All&page=0", 1800, "random"),
        ("https://www.semafor.com/newsletters/business/latest", 1100, "random"),
        ("https://www.semafor.com/rss.xml", 1100, "random"),
        ("https://www.sprucepointcap.com/research/feed/", 1100, "random"),
        ("https://www.statnews.com/feed", 1100, "random"),
        ("https://www.statnews.com/staff/adam-feuerstein/feed", 1100, "random"),
        ("https://www.take2games.com/ir/press-releases", 1800, "random"),
        ("https://www.theinformation.com/feed", 1100, "random"),
        ("https://www.wolfpackresearch.com/items", 1100, "random"),
        ("https://www.youtube.com/feeds/videos.xml?channel_id=UC6VcWc1rAoWdBCM0JxrRQ3A", 1100, "random"),
        ("https://nasdaqtrader.com/Trader.aspx?id=archiveheadlines&cat_id=105", 1800, "random"),
        ("https://origin.kerrisdalecap.com/feed/", 1100, "random"),
        ("https://whitediamondresearch.com/", 1100, "random"),
        ("https://www.betaville.co.uk/exclusives", 1100, "random"),
        ("https://www.cadc.uscourts.gov/internet/home.nsf/uscadcnews.xml", 1800, "random"),
        ("https://www.nyc.gov/office-of-the-mayor/news.page", 1800, "random"),
        ("https://www.youtube.com/feeds/videos.xml?channel_id=UC0patpmwYbhcEUap0bTX3JQ", 1100, "random")
    ];
    
    // Insert each default site
    for (url, interval_secs, style) in default_sites {
        // We ignore errors - sites might already exist in DB
        let _ = sqlx::query!(
            "INSERT OR IGNORE INTO sites (url, interval_secs, style) VALUES (?1, ?2, ?3)",
            url,
            interval_secs,
            style
        )
        .execute(pool)
        .await;
    }
}

// Emergency reset endpoint to help with site deletion issues
async fn get_full_content(data: web::Data<AppState>, path: web::Path<(i64, String)>) -> impl Responder {
    let (site_id, timestamp) = path.into_inner();
    
    // Parse the timestamp
    let timestamp = DateTime::parse_from_rfc3339(&timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    
    // Fetch the content from the database
    let content = sqlx::query!(
        "SELECT content FROM updates WHERE site_id = ?1 AND timestamp = ?2 LIMIT 1",
        site_id,
        timestamp
    )
    .fetch_optional(&data.pool)
    .await;
    
    match content {
        Ok(Some(record)) => {
            HttpResponse::Ok().json(serde_json::json!({
                "content": record.content
            }))
        },
        Ok(None) => {
            HttpResponse::NotFound().body("Content not found")
        },
        Err(e) => {
            HttpResponse::InternalServerError().body(format!("Database error: {}", e))
        }
    }
}

async fn reset_db(data: web::Data<AppState>) -> impl Responder {
    println!("Emergency database reset requested");
    
    // Make sure foreign keys are enabled
    let _ = sqlx::query("PRAGMA foreign_keys = ON;").execute(&data.pool).await;
    
    // Complete reset by dropping and recreating tables
    println!("Dropping all tables...");
    let _ = sqlx::query("DROP TABLE IF EXISTS updates;").execute(&data.pool).await;
    let _ = sqlx::query("DROP TABLE IF EXISTS sites;").execute(&data.pool).await;
    
    // Recreate the schema
    println!("Recreating tables...");
    let sites_table = sqlx::query(
        "CREATE TABLE sites(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            interval_secs INTEGER NOT NULL,
            style TEXT NOT NULL,
            last_checked TEXT,
            last_updated TEXT,
            status TEXT
        );"
    ).execute(&data.pool).await;
    
    let updates_table = sqlx::query(
        "CREATE TABLE updates(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            site_id INTEGER,
            timestamp TEXT,
            diff_hash TEXT,
            content TEXT,
            FOREIGN KEY(site_id) REFERENCES sites(id) ON DELETE CASCADE
        );"
    ).execute(&data.pool).await;
    
    match (sites_table, updates_table) {
        (Ok(_), Ok(_)) => {
            // Ensure foreign keys are enabled
            let _ = sqlx::query("PRAGMA foreign_keys = ON;").execute(&data.pool).await;
            
            // Re-add default sites
            add_default_sites(&data.pool).await;
            println!("Database has been reset successfully and default sites added");
            HttpResponse::Ok().body("Database has been completely reset. All tables were recreated and default sites were added.")
        },
        (Err(e), _) | (_, Err(e)) => {
            println!("Error resetting database: {}", e);
            HttpResponse::InternalServerError().body(format!("Error resetting database: {}", e))
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    // load config
    let cfg: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string("config.yaml").unwrap()).unwrap();
    let db_url = cfg["database_url"].as_str().unwrap();
    
    // Parse other config values
    let app_config = AppConfig {
        update_cache_size: cfg["update_cache_size"].as_i64().unwrap_or(5),
        default_interval_secs: cfg["default_interval_secs"].as_i64().unwrap_or(1),
        interval_jitter_max_ms: cfg["interval_jitter_max_ms"].as_i64().unwrap_or(1500),
    };
    
    println!("Config loaded: {:?}", app_config);
    
    let pool = SqlitePool::connect(db_url).await.expect("DB connect");

    // ensure schema
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sites(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            interval_secs INTEGER NOT NULL,
            style TEXT NOT NULL,
            last_checked TEXT,
            last_updated TEXT,
            status TEXT
         );"
    ).execute(&pool).await.unwrap();

    // Reset tables if requested via environment variable (for testing/development)
    let should_add_default_sites = if std::env::var("RESET_DB").is_ok() {
        println!("RESET_DB environment variable detected. Dropping all tables...");
        sqlx::query("DROP TABLE IF EXISTS updates;").execute(&pool).await.unwrap();
        sqlx::query("DROP TABLE IF EXISTS sites;").execute(&pool).await.unwrap();
        println!("Tables dropped. Will recreate them now.");
        true
    } else {
        // Check if there are any sites - if not, consider this a fresh install
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sites")
            .fetch_optional(&pool)
            .await
            .unwrap_or_else(|_| Some((0,)))
            .unwrap_or((0,));
        
        count.0 == 0
    };
    
    // Enable foreign key constraints in SQLite - MUST be set for each connection
    sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await.unwrap();
    
    // Make sure we recreate the tables with proper constraints
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS updates(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            site_id INTEGER,
            timestamp TEXT,
            diff_hash TEXT,
            content TEXT,
            FOREIGN KEY(site_id) REFERENCES sites(id) ON DELETE CASCADE
        );"
    ).execute(&pool).await.unwrap();
    
    // Double-check that foreign keys are enabled
    sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await.unwrap();
    
    // Check if foreign keys are actually enforced
    let fk_check: (i64,) = sqlx::query_as("PRAGMA foreign_keys;")
        .fetch_one(&pool)
        .await
        .unwrap();
        
    println!("Foreign key constraints enabled: {}", if fk_check.0 == 1 { "yes" } else { "no" });

    // Add default sites if needed
    if should_add_default_sites {
        println!("Adding default sites to the database...");
        add_default_sites(&pool).await;
        println!("Default sites added successfully");
    }

    let (tx, _rx) = broadcast::channel(1000);
    let state = Arc::new(AppState { 
        pool: pool.clone(), 
        tx_updates: tx.clone(),
        config: app_config.clone()
    });

    // spawn scraper background task
    tokio::spawn(scraper::run_scraper(pool.clone(), tx.clone(), app_config.clone()));

    // start HTTP server
    println!("Starting HTTP server at http://0.0.0.0:8080");
    println!("Open your browser at http://localhost:8080");
    println!("Press Ctrl+C to stop the server");
    
    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::from(state.clone()))
            .service(web::resource("/api/sites").route(web::get().to(list_sites)).route(web::post().to(add_site)))
            .service(web::resource("/api/sites/{id}").route(web::delete().to(delete_site)))
            .service(web::resource("/api/updates/stream").route(web::get().to(sse_updates)))
            .service(web::resource("/api/reset-db").route(web::get().to(reset_db)))
            .service(web::resource("/api/content/{site_id}/{timestamp}").route(web::get().to(get_full_content)))
            .service(Files::new("/", "./static").index_file("index.html"))
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}