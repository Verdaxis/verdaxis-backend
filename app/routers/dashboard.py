import psutil
import time
import subprocess
import platform
import os
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from typing import List, Optional

router = APIRouter(
    prefix="/dashboard",
    tags=["dashboard"]
)

class SystemHealth(BaseModel):
    status: str
    uptime_seconds: float
    cpu_usage_pct: float
    memory_usage_pct: float
    disk_free_gb: float
    disk_used_pct: float
    platform: str

class LogEntry(BaseModel):
    timestamp: str
    level: str
    message: str

@router.get("/health", response_model=SystemHealth)
async def get_system_health():
    """
    Get current system health metrics.
    """
    try:
        # Boot time
        boot_time = psutil.boot_time()
        uptime = time.time() - boot_time
        
        # CPU
        cpu_pct = psutil.cpu_percent(interval=None) # Non-blocking
        
        # Memory
        mem = psutil.virtual_memory()
        
        # Disk
        disk = psutil.disk_usage('/')
        
        return SystemHealth(
            status="healthy",
            uptime_seconds=uptime,
            cpu_usage_pct=cpu_pct,
            memory_usage_pct=mem.percent,
            disk_free_gb=round(disk.free / (1024**3), 2),
            disk_used_pct=disk.percent,
            platform=platform.system()
        )
    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))

@router.get("/logs")
async def get_logs(lines: int = 50):
    """
    Get recent system logs. 
    In local dev (Mac/Linux without journalctl), this might be limited.
    We'll attempt to Read from common log files or return a mock if unavailable.
    """
    logs = []
    
    # Try reading a dummy log file if exists, or a specific app log
    # For now, let's just return some system "dmesg" or similar if possible, 
    # but for safety and relevance, maybe just list running processes?
    # Or actually, let's try to mock it if we can't find a real log source,
    # as the user is likely running locally.
    
    # Attempt to read 'verdaxis.log' if it exists in root
    log_file_path = "verdaxis.log"
    if os.path.exists(log_file_path):
        try:
            # Simple tail implementation
            with open(log_file_path, "r") as f:
                content = f.readlines()
                logs = content[-lines:]
                return {"logs": [line.strip() for line in logs]}
        except Exception:
            pass

    # Fallback: Just return a message saying logs are only available if configured
    return {
        "logs": [
            f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] INFO System health check initiated",
            f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] INFO CPU Usage: {psutil.cpu_percent()}%",
            f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] INFO Memory Usage: {psutil.virtual_memory().percent}%",
            f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] WARN  Real log file 'verdaxis.log' not found.",
        ]
    }
