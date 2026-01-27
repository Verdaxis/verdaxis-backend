# Verdaxis Intelligence Cockpit - Backend

The backend service for the Verdaxis platform, a maritime intelligence and procurement system. This service provides APIs for fuel procurement, compliance auditing (EU ETS, FuelEU), port intelligence, and AI-driven insights.

## Features

- **Authentication**: User and Organization management with JWT-based auth.
- **Port Intelligence**: Geospatial data for ports, including congestion and resource availability.
- **Marketplace**: Bunkering quote requests, supplier negotiation, and inventory management.
- **Compliance**: Tracking and auditing for maritime regulations (EU ETS, FuelEU).
- **AI Integration**: Generative AI support for market analysis and decision support using Google Gemini.

## Technology Stack

- **Framework**: FastAPI (Python 3.10+)
- **Database**: PostgreSQL with PostGIS extension (via `asyncpg` and `SQLAlchemy`)
- **Migrations**: Alembic
- **AI**: Google Generative AI SDK
- **Geospatial**: GeoAlchemy2
- **Containerization**: Docker & Docker Compose

## Getting Started

### Prerequisites

- Python 3.10+
- Docker & Docker Compose
- PostgreSQL (if running locally without Docker)

### Environment Setup

1.  Copy the example environment file:
    ```bash
    cp .env.example .env
    ```
2.  Configure your variables in `.env` (Database URL, API Keys, etc.).

### Running Locally

**Using Docker (Recommended):**

```bash
docker-compose up --build
```

**Manual Setup:**

1.  Create a virtual environment:
    ```bash
    python -m venv venv
    source venv/bin/activate
    ```
2.  Install dependencies:
    ```bash
    pip install -r requirements.txt
    ```
3.  Run migrations:
    ```bash
    alembic upgrade head
    ```
4.  Start the server:
    ```bash
    uvicorn app.main:app --reload
    ```

The API will be available at `http://localhost:8000`.
API Documentation (Swagger UI) is at `http://localhost:8000/docs`.

## Project Structure

- `app/main.py`: Application entry point.
- `app/models/`: Database models (SQLAlchemy).
- `app/schemas/`: Pydantic schemas for request/response validation.
- `app/routers/`: API route definitions.
- `app/services/`: Business logic and external integrations.
- `app/core/`: Core configuration and security utilities.
- `alembic/`: Database migration scripts.
