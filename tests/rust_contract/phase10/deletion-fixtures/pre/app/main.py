"""Synthetic Python application entry point (fixture data only; never run)."""
from app.admin import setup_admin
from app.mcp_server import MCP_PATH, build_mcp_http_app, build_mcp_server
from app.routers.activity import router as activity_router
from app.routers.ai import router as ai_router

app.include_router(activity_router, prefix="/api")
app.include_router(ai_router, prefix="/api")
setup_admin(app)
mcp = build_mcp_server()
app.mount(MCP_PATH, build_mcp_http_app(mcp))
