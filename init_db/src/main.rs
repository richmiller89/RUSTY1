use std::env;
use sqlx::{SqlitePool, migrate::MigrateDatabase, Sqlite};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get the root directory of the workspace
    let current_dir = env::current_dir().expect("Failed to get current dir");
    let workspace_root = if current_dir.ends_with("init_db") {
        current_dir.parent().expect("Failed to get parent dir").to_path_buf()
    } else {
        current_dir
    };
    
    // Create the database file in the workspace root
    let db_path = workspace_root.join("scraper.db");
    let db_url = format!("sqlite:{}", db_path.display());
    
    // Create the database if it doesn't exist
    if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
        println!("Creating database at: {}", db_url);
        Sqlite::create_database(&db_url).await?;
    } else {
        println!("Database already exists at: {}", db_url);
    }
    
    // Connect to the database
    let pool = SqlitePool::connect(&db_url).await?;
    
    // Create tables
    println!("Creating tables...");
    
    // Create sites table
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
    ).execute(&pool).await?;
    
    // Create updates table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS updates(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            site_id INTEGER,
            timestamp TEXT,
            diff_hash TEXT,
            content TEXT,
            FOREIGN KEY(site_id) REFERENCES sites(id)
        );"
    ).execute(&pool).await?;
    
    // Close the connection
    pool.close().await;
    
    println!("Database initialized successfully at: {}", db_url);
    println!("You can now build and run the application with:");
    println!("set DATABASE_URL=sqlite:scraper.db");
    println!("cargo run");
    
    Ok(())
}