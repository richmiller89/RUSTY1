# Rust Web Scraper

## Overview
A high‑performance website watcher written in Rust that monitors websites for changes and notifies you in real-time when content updates are detected.

### Features

* Async scraping with Tokio + Reqwest
* Configurable intervals per site (1–3000 s) with random jitter or exponential back‑off
* SQLite persistence via SQLx
* Real‑time updates pushed to the browser via Server‑Sent Events
* Simple front‑end (vanilla JS) for configuration and live feed
* Handles 100+ site checks/sec on modest hardware

## Quick Start (Windows)

```bat
:: Install Rust stable if you haven't already
curl https://win.rustup.rs -sSf | sh

:: Simply use the batch file (recommended)
run_scraper.bat
```

Then open `http://localhost:8080` in your browser.

The included `run_scraper.bat` handles all the setup automatically and is the recommended way to run the application.

## Configuration
Edit `config.yaml` before first run:

* `database_url` – Use format `sqlite:scraper.db` (single colon, not double)
* `update_cache_size` – Number of body snapshots per site to cache (default: 5)
* `default_interval_secs` – Default poll interval for newly added sites (default: 1 second)
* `interval_jitter_max_ms` – Maximum random delay added per poll for the random style (default: 1500ms)

## Scraping Styles

The application supports three different scraping styles:

1. **Random** - Adds random jitter to the polling interval, within the configured range
   - Example: With interval of 5s and jitter of 1500ms, the site will be checked every 5-6.5 seconds
   - Useful for preventing predictable server load patterns

2. **Exponential** - Uses exponential back-off on errors, doubling the wait time on each failure
   - Example: With interval of 5s, after a failure it will wait 10s, then 20s, etc.
   - After a successful check, resets to the configured interval
   - Useful for sites that might temporarily block frequent requests

3. **None** - Uses fixed interval with no jitter or back-off
   - Example: With interval of 5s, the site will be checked exactly every 5 seconds
   - Simplest approach, but less resilient to temporary failures

## Usage

1. Open the application in your browser at http://localhost:8080
2. Add sites to monitor:
   - Enter the URL
   - Set the check interval (1-3000 seconds)
   - Choose a scraping style (random, exponential, or none)
3. The application will begin monitoring the sites immediately
4. Live updates will appear in the "Live Updates" section when changes are detected
5. Site status, last check time, and last update time are displayed in the table

## Data Storage

The application stores the following information:

1. **Site Configuration:**
   - URL, polling interval, scraping style
   - Status (OK or ERROR)
   - Last check and last update timestamps

2. **Site Updates:**
   - Content snapshots when changes are detected
   - SHA-256 hash of the content for change detection
   - Limited to the configured number of updates per site

## Development Notes

### SQLx Setup for Compilation

SQLx requires a database with the correct schema to validate queries during compilation:

1. Create a `.env` file in the project root with:
   ```
   DATABASE_URL=sqlite:scraper.db
   ```

2. Initialize the database with the schema:
   ```
   cargo run --bin init_db
   ```
   
3. When building or running, ensure the environment variable is set:
   ```
   set DATABASE_URL=sqlite:scraper.db
   cargo build
   ```

### Running in Development Mode

For development with more verbose logging:

```
run_scraper_dev.bat
```