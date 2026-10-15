import os
import time
from secrets import compare_digest, token_hex

import psutil
import sqladmin
from sqladmin import Admin, BaseView, ModelView, expose
from sqladmin.authentication import AuthenticationBackend
from sqlalchemy import text
from starlette.requests import Request
from starlette.templating import Jinja2Templates

from app.config import settings
from app.database import engine
from app.models.user import Organization, User

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

authentication_backend = AdminAuth(secret_key=settings.ADMIN_SESSION_SECRET or "disabled")

class RetentionAwareDelete:
    """SQLAdmin displays the stable conflict in its list-page error banner."""

    async def delete_model(self, request, pk):
        from uuid import UUID

        from sqlalchemy import select

        from app.services.kyc_document_retention import require_no_retained_documents

        async with self.session_maker() as session:
            obj = await session.scalar(select(self.model).where(
                self.model.id == UUID(str(pk))
            ).with_for_update())
            if obj is None:
                return
            if self.model is User:
                await require_no_retained_documents(session, user_id=obj.id)
            else:
                await require_no_retained_documents(session, organization_id=obj.id)
            await self.on_model_delete(obj, request)
            await session.delete(obj)
            await session.commit()
            await self.after_model_delete(obj, request)


def setup_admin(app):
    if not settings.ENABLE_SQLADMIN:
        return
    if not settings.ADMIN_SESSION_SECRET:
        raise RuntimeError("ENABLE_SQLADMIN requires ADMIN_SESSION_SECRET")
    admin = Admin(app, engine, authentication_backend=authentication_backend)

    class OrganizationAdmin(RetentionAwareDelete, ModelView, model=Organization):
        column_list = [Organization.id, Organization.name, Organization.type, Organization.supplier_tier]

    class UserAdmin(RetentionAwareDelete, ModelView, model=User):
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
