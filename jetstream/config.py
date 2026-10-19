"""Configuration settings for JetStream API."""

from pydantic import Field
from pydantic_settings import BaseSettings
from pathlib import Path
from typing import Optional
import os
import sys

LEGACY_DATABASE_FILENAME = "jetstream.db"


def user_data_dir() -> Path:
    """Per-user data directory, independent of the folder JetStream is started from."""
    override = os.environ.get("JETSTREAM_DATA_DIR")
    if override:
        return Path(override).expanduser()
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or os.path.join(Path.home(), "AppData", "Local")
        return Path(base) / "JetStream"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "JetStream"
    base = os.environ.get("XDG_DATA_HOME") or os.path.join(Path.home(), ".local", "share")
    return Path(base) / "jetstream"


def default_database_path() -> Path:
    return user_data_dir() / LEGACY_DATABASE_FILENAME


def default_database_url() -> str:
    return f"sqlite:///{default_database_path().as_posix()}"

class Settings(BaseSettings):
    """Application settings."""
    
    # Server settings
    HOST: str = "0.0.0.0"
    PORT: int = 8000
    DEBUG: bool = False
    AUTO_OPEN_BROWSER: bool = True  # Auto-open browser on startup
    # Base URL used to build OAuth callback redirect URI.
    # Set this to the URL users use to reach JetStream (e.g. http://myserver:8000).
    JETSTREAM_BASE_URL: str = "http://localhost:8000"
    
    # Database (defaults to the per-user data dir so history doesn't depend on the launch folder)
    DATABASE_URL: str = Field(default_factory=default_database_url)
    
    # Google Cloud Storage
    GCS_PROJECT_ID: Optional[str] = None
    GCS_CREDENTIALS_PATH: Optional[str] = None
    GCS_BUCKET_NAME: str = "nmfs_odp_pifsc"  # Default bucket name
    GCS_QUARANTINE_BUCKET: Optional[str] = None
    GCS_PROTECTED_PREFIXES: list = ["_critical/", "do-not-touch/"]

    # Cloud audit defaults
    # Regexes matched (case-insensitively) against object names. Keep in sync with
    # DEFAULT_PATTERNS in desktop/transfer/src/audit.rs.
    GCS_JUNK_REGEX_PATTERNS: list = [
        r"\.gstmp$",
        r"(^|/)@eaDir/",
        r"(^|/)Thumbs\.db$",
        r"(^|/)\.DS_Store$",
        r"(^|/)desktop\.ini$",
        r"(^|/)~\$",
        r"\.(tmp|temp|crdownload|part)$",
    ]
    GCS_AUDIT_SCAN_MAX_OBJECTS: int = 250000
    GCS_AUDIT_FINDINGS_PAGE_SIZE: int = 200

    # Dataset creator defaults
    DATASET_CREATOR_MAX_SCAN_RESULTS: int = 50000
    # Bucket allowlist for scan/proxy. "*" allows any bucket the credentials can
    # reach; set a comma-separated list to restrict (recommended if exposed off localhost).
    DATASET_CREATOR_ALLOWED_BUCKETS: str = "*"
    DATASET_CREATOR_VIEWER_TTL_SECONDS: int = 3600
    DATASET_CREATOR_PREVIEW_LIMIT: int = 200

    # Local audit defaults
    LOCAL_AUDIT_MAX_DETAILED_FILES: int = 20000
    LOCAL_AUDIT_SCAN_MAX_SECONDS: int = 120
    LOCAL_AUDIT_DOCS_DOMINANCE_PCT: float = 0.60
    LOCAL_AUDIT_MEDIA_DOMINANCE_PCT: float = 0.60
    LOCAL_AUDIT_ARCHIVE_AGE_DAYS: int = 365
    LOCAL_AUDIT_ARCHIVE_MIN_SIZE_MB: int = 250
    
    # Scanner Performance Settings (Optimized for very large folders)
    MAX_FILES_FOR_DETAILED_SCAN: int = 1000   # Switch to folder-only after this
    MAX_SUBFOLDERS_FOR_DETAILED_SCAN: int = 20  # Switch to folder-only after this
    SCAN_TIMEOUT_SECONDS: int = 60  # Max time for a scan (1 minute)
    ENABLE_FAST_SCAN: bool = True  # Use os.scandir instead of os.walk
    
    # File filtering (from jetstream.py)
    EXCLUDE_FOLDERS: list = [
        "_archive", "_YEAR", "ISLAND", "SITE-ID", "SITE_PHOTOS", 
        "Corrected", "corrected", "uncorrected", "MISC", "DARK", 
        "Products", "Thumbs.db", ".DS_Store", "__pycache__"
    ]
    
    EXCLUDE_PATTERNS: list = [
        r'.*\.tmp$',
        r'.*\.bak$',
        r'.*~$',
        r'.*\.pyc$',
    ]
    
    INCLUDE_PATTERNS: list = [
        r'^.*(?<!\.JPG)$',
        r'^.*\.(jpg|jpeg|png|tiff|tif|raw|cr2|nef|arw|dng)$',
        r'^.*\.(mp4|mov|avi|mkv)$',
        r'^.*\.(txt|csv|json|xml|log)$'
    ]

    # Native Google Drive API (gdrive page)
    # Create a Web Application OAuth 2.0 client at https://console.cloud.google.com/apis/credentials
    # and set redirect URI to {JETSTREAM_BASE_URL}/api/gdrive/auth/callback
    GDRIVE_CLIENT_ID: Optional[str] = None
    GDRIVE_CLIENT_SECRET: Optional[str] = None
    # Allow OAuth over HTTP only for local/dev scenarios.
    GDRIVE_ALLOW_INSECURE_OAUTH: bool = False
    # Security: Drive OAuth tokens are process-memory only and are never persisted to disk.
    # Bound in-memory OAuth state entries to reduce abuse/memory growth.
    GDRIVE_OAUTH_STATE_MAX_ENTRIES: int = 1024

    # Comma-separated CORS origins. Avoid "*" when credentials are enabled.
    CORS_ALLOW_ORIGINS: str = "http://localhost:8000,http://127.0.0.1:8000"
    CORS_ALLOW_CREDENTIALS: bool = True

    class Config:
        env_file = ".env"
        case_sensitive = True

settings = Settings()
