#!/bin/bash
# deploy.sh - Server-side deployment script for Verdaxis Backend
# Run this on the VPS to pull latest code and redeploy
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Verdaxis Backend Deployment ==="
echo "Timestamp: $(date)"

cd "$BACKEND_DIR"

# Pull latest code
echo ""
echo ">>> Pulling latest code from origin..."
git fetch origin
git checkout main
git pull origin main

# Rebuild and restart containers
echo ""
echo ">>> Rebuilding Docker containers..."
docker compose down || true
docker compose up -d --build --remove-orphans

# Wait for containers to be healthy
echo ""
echo ">>> Waiting for containers to start..."
sleep 5

# Run database migrations
echo ""
echo ">>> Running database migrations..."
docker exec verdaxis-backend alembic upgrade head || echo "Migration failed or no migrations needed"

# Run seed script if needed
echo ""
echo ">>> Checking if seed data exists..."
docker exec verdaxis-backend python -c "
from app.database import AsyncSessionLocal
from app.models.user import User
from sqlalchemy import select
import asyncio

async def check():
    async with AsyncSessionLocal() as db:
        result = await db.execute(select(User).limit(1))
        return result.scalar_one_or_none()

user = asyncio.run(check())
if not user:
    print('No users found, running seed...')
    exit(1)
else:
    print('Seed data exists, skipping...')
    exit(0)
" && echo "Seed data present" || docker exec verdaxis-backend python scripts/seed.py

# Health check
echo ""
echo ">>> Running health check..."
sleep 2
curl -s http://localhost:8000/health | grep -q "ok" && echo "✓ Backend is healthy!" || echo "✗ Backend health check failed"

echo ""
echo "=== Backend Deployment Complete ==="
