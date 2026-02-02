from sqladmin import Admin, ModelView, BaseView, expose
from sqladmin.authentication import AuthenticationBackend
from starlette.requests import Request
from starlette.responses import RedirectResponse
from app.database import engine
from app.models.user import User, Organization
from app.models.orders import PublicListing, Order
from app.models.marketplace import QuoteRequest, QuoteOffer
from app.config import settings
import psutil
import time
import os
import sqladmin
from starlette.templating import Jinja2Templates
from sqlalchemy import text

sqladmin_path = os.path.dirname(sqladmin.__file__)
templates = Jinja2Templates(directory=["templates", os.path.join(sqladmin_path, "templates")])

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
                "logs": logs,
                "admin": self.admin
            }
        )

class AdminAuth(AuthenticationBackend):
    async def login(self, request: Request) -> bool:
        form = await request.form()
        username = form.get("username")
        password = form.get("password")

        if username == "admin" and password == "admin":  # TODO: Change this to real auth
            request.session.update({"token": "admin-token"})
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

authentication_backend = AdminAuth(secret_key=settings.JWT_SECRET)

def setup_admin(app):
    admin = Admin(app, engine, authentication_backend=authentication_backend)

    class OrganizationAdmin(ModelView, model=Organization):
        column_list = [Organization.id, Organization.name, Organization.type]

    class UserAdmin(ModelView, model=User):
        column_list = [User.id, User.email, User.first_name, User.role]

    class ListingAdmin(ModelView, model=PublicListing):
        name = "Public Listing"
        name_plural = "Public Listings"
        column_list = [PublicListing.id, PublicListing.fuel_type, PublicListing.price_per_mt_usd, PublicListing.region]

    class QuoteRequestAdmin(ModelView, model=QuoteRequest):
        column_list = [QuoteRequest.id, QuoteRequest.fuel_type, QuoteRequest.quantity_mt]
    
    class QuoteOfferAdmin(ModelView, model=QuoteOffer):
        column_list = [QuoteOffer.id, QuoteOffer.price_per_mt_usd]

    class OrderAdmin(ModelView, model=Order):
        name = "Market Order"
        name_plural = "Market Orders"
        column_list = [Order.id, Order.status]

    admin.add_view(OrganizationAdmin)
    admin.add_view(UserAdmin)
    admin.add_view(ListingAdmin)
    admin.add_view(QuoteRequestAdmin)
    admin.add_view(QuoteOfferAdmin)
    admin.add_view(OrderAdmin)
    
    # Inject admin instance into view class
    SystemHealthView.admin = admin
    admin.add_view(SystemHealthView)
