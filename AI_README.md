# AI Context: Verdaxis Backend

> **Purpose**: This file provides structural and contextual information for AI agents working on this codebase.

## Project Context

Verdaxis is a maritime platform designed to modernize fuel procurement and compliance. The backend serves as the central intelligence hub, aggregation data from geospatial sources, compliance registries, and market participants.

## Architecture Guidelines

- **Framework**: FastAPI. Designed for high performance and async I/O.
- **Database**:
  - Uses **SQLAlchemy v2** with `AsyncSession`.
  - **PostGIS** is used for geospatial queries (e.g., finding nearby ports).
  - **Alembic** handles schema migrations.
- **Asynchronous**: All I/O bound operations (DB, API calls) must be `async`.

## Key Directory Structure

| Directory | Purpose |
|Path|Description|
|---|---|
| `app/routers/` | **Controllers**. Define endpoints. Logic should be minimal here, delegating to services/CRUD. |
| `app/models/` | **DB Code**. SQLAlchemy ORM definitions. Maps to DB tables. |
| `app/schemas/` | **DTOs**. Pydantic models for data validation and sterilization. |
| `app/services/` | **Business Logic**. Complex operations, AI calls, and third-party integrations. |
| `app/core/` | **Config**. Settings, Security, and Database connection setup. |

## Core Data Models

- **User / Organization**: Users belong to Organizations (Buyer or Supplier).
- **Vessel**: Core asset for Buyers. Has geospatial/tracking data.
- **Port**: Geospatial entity. Uses PostGIS `Geometry` types.
- **QuoteRequest**: Central entity for the Marketplace. Links Buyers, Vessels, and Ports to Suppliers.
- **ComplianceRecord**: Stores EU ETS/FuelEU verification data.

## Coding Conventions

1.  **Strict Typing**: Use Python type hints everywhere.
2.  **Pydantic**: Use `ConfigDict(from_attributes=True)` (formerly `orm_mode`) for DB-to-JSON conversion.
3.  **Dependency Injection**: Use `Depends()` for DB sessions (`get_db`) and current user (`get_current_user`).
4.  **Error Handling**: Raise `HTTPException` with clear detail strings.

## AI Integration

- **Router**: `app/routers/ai.py`
- **Usage**: The AI service (Google GenAI) is used for:
  - Analyzing market trends from text.
  - Optimizing fuel procurement strategies.
  - Summarizing compliance reports.

## Deployment Access

- **VPS Host**: 144.126.151.136
- **User**: verdaxis-prod
- **Command**: `ssh verdaxis-prod@144.126.151.136`
