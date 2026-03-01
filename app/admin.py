from sqladmin import Admin, ModelView, BaseView, expose
from sqladmin.authentication import AuthenticationBackend
from starlette.requests import Request
from starlette.responses import RedirectResponse
from app.database import engine
from app.models.user import User, Organization
from app.config import settings
import psutil
import time
import os
import sqladmin
from secrets import compare_digest, token_hex
from starlette.templating import Jinja2Templates
from sqlalchemy import text

sqladmin_path = os.path.dirname(sqladmin.__file__)
templates = Jinja2Templates(directory=["templates", os.path.join(sqladmin_path, "templates")])

class AdminAuth(AuthenticationBackend):
    async def login(self, request: Request) -> bool:
        form = await request.form()
        username = form.get("username")
        password = form.get("password")

        configured_username = settings.ADMIN_USERNAME
        configured_password = settings.ADMIN_PASSWORD
        if not configured_username or not configured_password:
            return False

        if compare_digest(username or "", configured_username) and compare_digest(password or "", configured_password):
            request.session.update({"token": token_hex(32)})
            return True
        return False

    async def logout(self, request: Request) -> bool:
        request.session.clear()
        return True

    async def authenticate(self, request: Request) -> bool:
        token = request.session.get("token")
        if not token:
            return False
        return True

authentication_backend = AdminAuth(secret_key=settings.ADMIN_SESSION_SECRET or settings.JWT_SECRET)

def setup_admin(app):
    admin = Admin(app, engine, authentication_backend=authentication_backend)

    class OrganizationAdmin(ModelView, model=Organization):
        column_list = [Organization.id, Organization.name, Organization.type, Organization.supplier_tier]

    class UserAdmin(ModelView, model=User):
        column_list = [User.id, User.email, User.first_name, User.role]

    class SystemHealthView(BaseView):
        name = "System Health"
        icon = "fa-solid fa-heart-pulse"

        async def check_db(self):
            try:
                async with engine.connect() as conn:
                    await conn.execute(text("SELECT 1"))
                return True
            except Exception:
                return False

        @expose("/health", methods=["GET"])
        async def health_page(self, request):
            cpu = psutil.cpu_percent()
            mem = psutil.virtual_memory()
            disk = psutil.disk_usage('/')
            
            uptime_seconds = time.time() - psutil.boot_time()
            days = int(uptime_seconds // (24 * 3600))
            hours = int((uptime_seconds % (24 * 3600)) // 3600)
            minutes = int((uptime_seconds % 3600) // 60)
            uptime_str = f"{days}d {hours}h {minutes}m"

            db_status = "OK" if await self.check_db() else "Error"

            logs = ["Log viewing disabled in this version"]

            return templates.TemplateResponse(
                "system_health.html",
                {
                    "request": request,

                    "stats": {
                        "cpu": cpu,
                        "memory": {
                            "percent": mem.percent,
                            "used_gb": round(mem.used / (1024**3), 1),
                            "total_gb": round(mem.total / (1024**3), 1)
                        },
                        "disk": {
                            "percent": disk.percent,
                            "free_gb": round(disk.free / (1024**3), 1),
                            "total_gb": round(disk.total / (1024**3), 1)
                        },
                        "uptime": uptime_str,
                        "db_status": db_status
                    },
                    "logs": logs,
                    "admin": admin
                }
            )

    admin.add_view(OrganizationAdmin)
    admin.add_view(UserAdmin)
    admin.add_view(SystemHealthView)
