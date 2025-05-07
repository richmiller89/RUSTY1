@echo off
:: Compile and run the scraper in development mode with verbose output
cd /d %~dp0

:: Set environment variables with SETLOCAL to ensure they persist
SETLOCAL

:: Make sure the database path is absolute
set DATABASE_URL=sqlite:%~dp0scraper.db
:: Set RUST_LOG for verbose logging
set RUST_LOG=debug,actix_web=debug,sqlx=debug

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
echo Starting scraper in development mode:
echo DATABASE_URL=%DATABASE_URL%
echo RUST_LOG=%RUST_LOG%
echo.
echo Server is running at http://localhost:8080
echo Press Ctrl+C to stop the server
echo.

:: Run with the environment variables explicitly set in the same command
set "DATABASE_URL=%DATABASE_URL%" && set "RUST_LOG=%RUST_LOG%" && cargo run

:: This will only execute if the application terminates
echo Server has been stopped.
pause