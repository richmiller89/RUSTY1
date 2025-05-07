CREATE TABLE IF NOT EXISTS sites(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL UNIQUE,
    interval_secs INTEGER NOT NULL,
    style TEXT NOT NULL,
    last_checked TEXT,
    last_updated TEXT,
    status TEXT
);

CREATE TABLE IF NOT EXISTS updates(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    site_id INTEGER,
    timestamp TEXT,
    diff_hash TEXT,
    content TEXT,
    FOREIGN KEY(site_id) REFERENCES sites(id)
);