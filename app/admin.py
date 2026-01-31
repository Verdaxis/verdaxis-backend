from sqladmin import Admin, ModelView, BaseView, expose
from sqladmin.authentication import AuthenticationBackend
from starlette.requests import Request
from starlette.responses import RedirectResponse
from app.database import engine
from app.models.user import User
from app.models.user import User, Organization
from app.models.rfq import PublicListing, RFQMatch
from app.models.marketplace import QuoteRequest, QuoteOffer
from app.config import settings
import psutil
import time
import os

from starlette.templating import Jinja2Templates

templates = Jinja2Templates(directory="templates")

class SystemHealthView(BaseView):
    name = "System Health"
    icon = "fa-solid fa-heart-pulse"

    @expose("/health", methods=["GET"])
    async def health_page(self, request):
        cpu = psutil.cpu_percent()
        mem = psutil.virtual_memory().percent
        disk = psutil.disk_usage('/').percent
        uptime = time.time() - psutil.boot_time()
        
        # Format uptime
        days = int(uptime // (24 * 3600))
        uptime = uptime % (24 * 3600)
        hours = int(uptime // 3600)
        uptime %= 3600
        minutes = int(uptime // 60)
        uptime_str = f"{days}d {hours}h {minutes}m"

        # Get logs
        logs = []
        log_file_path = "verdaxis.log"  # Or your actual log file
        if os.path.exists(log_file_path):
            try:
                with open(log_file_path, "r") as f:
                    logs = [line.strip() for line in f.readlines()[-50:]]
            except:
                logs = ["Error reading log file"]
        else:
             logs = ["Log file not found"]


        return templates.TemplateResponse(
            "system_health.html",
            {
                "request": request,
                "cpu_usage": cpu,
                "memory_usage": mem,
                "disk_usage": disk,
                "uptime": uptime_str,
                "logs": logs
            }
        )


class AdminAuth(AuthenticationBackend):
    async def login(self, request: Request) -> bool:
        form = await request.form()
        username = form.get("username")
        password = form.get("password")

        # Basic hardcoded admin check for now, or validate against DB
        # For simplicity in this demo, we can check against env vars or a specific user
        # In production, you'd verify against the User table with administrative privileges
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

    class RFQMatchAdmin(ModelView, model=RFQMatch):
        name = "Market Match"
        name_plural = "Market Matches"
        column_list = [RFQMatch.id, RFQMatch.status]

    admin.add_view(OrganizationAdmin)
    admin.add_view(UserAdmin)
    admin.add_view(ListingAdmin)
    admin.add_view(QuoteRequestAdmin)
    admin.add_view(QuoteOfferAdmin)
    admin.add_view(RFQMatchAdmin)
    admin.add_view(SystemHealthView)
