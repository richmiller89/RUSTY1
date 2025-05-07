@echo off
:: Compile and run the scraper in release mode
cd /d %~dp0

:: Set environment variable for SQLx (with SETLOCAL to ensure it persists)
SETLOCAL

:: Make sure the database path is absolute
set DATABASE_URL=sqlite:%~dp0scraper.db

:: Setup the SQLite database for SQLx
echo Initializing database...
cargo run -p init_db
if %ERRORLEVEL% NEQ 0 (
    echo Failed to initialize database. Error code: %ERRORLEVEL%
    pause
    exit /b %ERRORLEVEL%
)

:: Navigate to the backend directory
cd /d %~dp0scraper_backend
echo Starting scraper (DATABASE_URL=%DATABASE_URL%)...
echo.
echo Server is running at http://localhost:8080
echo Press Ctrl+C to stop the server
echo.

:: Run with the environment variable explicitly set in the same command
set "DATABASE_URL=%DATABASE_URL%" && cargo run --release

:: This will only execute if the application terminates
echo Server has been stopped.
pause