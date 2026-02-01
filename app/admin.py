from sqladmin import Admin, ModelView, BaseView, expose
from sqladmin.authentication import AuthenticationBackend
from starlette.requests import Request
from starlette.responses import RedirectResponse
from app.database import engine
from app.models.user import User
from app.models.user import User, Organization
from app.models.orders import PublicListing, Order

# ...

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
    admin.add_view(SystemHealthView)
