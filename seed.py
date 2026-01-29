import asyncio
import logging
import uuid
from sqlalchemy import select, text
from app.database import AsyncSessionLocal
from app.models.user import User, UserRole, Organization, OrgType, UserStatus
from app.models.port import Port, Vessel, CongestionLevel
from app.models.marketplace import InventoryItem, FuelType
from app.models.rfq import PublicListing, FuelGrade, AvailabilityWindow, TierLabel, ListingStatus
from app.core.security import get_password_hash
from decimal import Decimal
from datetime import datetime, timedelta

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def seed():
    async with AsyncSessionLocal() as db:
        logger.info("Seeding data...")
        
        # Clean existing data to ensure fresh seed
        logger.info("Cleaning existing data...")
        await db.execute(text("TRUNCATE TABLE inventory_items, rfq_matches, commissions, public_listings, vessels, users, organizations, ports CASCADE"))
        await db.commit()
        
        logger.info("Creating data...")

        # --- Organizations ---
        # Buyers
        buyer_orgs = [
            Organization(name="Pacific Ocean Lines", type=OrgType.SHIPPING_LINE, country_code="SG"),
            Organization(name="Atlantic Cargo", type=OrgType.SHIPPING_LINE, country_code="US"),
            Organization(name="Nordic Shipping", type=OrgType.SHIPPING_LINE, country_code="NO")
        ]
        
        # Suppliers
        supplier_orgs = [
            Organization(name="Global Energy Supply", type=OrgType.FUEL_SUPPLIER, country_code="GB"),
            Organization(name="Eastern Bunkers", type=OrgType.FUEL_SUPPLIER, country_code="CN"),
            Organization(name="Future Fuels Ltd", type=OrgType.FUEL_SUPPLIER, country_code="NL")
        ]
        
        for o in buyer_orgs + supplier_orgs:
            db.add(o)
        await db.flush()

        # --- Users ---
        users = []
        # Admin
        users.append(User(
            email="admin@verdaxis.com", first_name="Admin", last_name="User",
            password_hash=get_password_hash("admin123"), role=UserRole.ADMIN, status=UserStatus.APPROVED
        ))

        # Buyers
        buyer_users = [
            ("Alice", "Wong", "manager@pacificoceanlines.com", buyer_orgs[0]),
            ("Bob", "Smith", "ops@atlanticcargo.com", buyer_orgs[1]),
            ("Charlie", "Hansen", "logistics@nordicshipping.com", buyer_orgs[2])
        ]

        for first, last, email, org in buyer_users:
            users.append(User(
                email=email, first_name=first, last_name=last,
                password_hash=get_password_hash("password123"), role=UserRole.BUYER,
                organization_id=org.id, status=UserStatus.APPROVED
            ))

        # Suppliers
        supplier_users = [
            ("David", "Jones", "sales@globalenergy.com", supplier_orgs[0]),
            ("Wei", "Chen", "bunkers@easternbunkers.cn", supplier_orgs[1]),
            ("Eva", "Muller", "contact@futurefuels.nl", supplier_orgs[2])
        ]

        for first, last, email, org in supplier_users:
            users.append(User(
                email=email, first_name=first, last_name=last,
                password_hash=get_password_hash("password123"), role=UserRole.SUPPLIER,
                organization_id=org.id, status=UserStatus.APPROVED
            ))
            
        for u in users:
            db.add(u)
        
        # --- Ports ---
        ports = [
            Port(id="NLRTM", name="Rotterdam", country="Netherlands", timezone="Europe/Amsterdam", location="POINT(4.47917 51.9225)", is_active=True),
            Port(id="SGSIN", name="Singapore", country="Singapore", timezone="Asia/Singapore", location="POINT(103.8198 1.3521)", is_active=True),
            Port(id="USHOU", name="Houston", country="USA", timezone="America/Chicago", location="POINT(-95.3698 29.7604)", is_active=True),
            Port(id="CNSHA", name="Shanghai", country="China", timezone="Asia/Shanghai", location="POINT(121.4737 31.2304)", is_active=True)
        ]
        for p in ports:
            db.add(p)
        await db.flush()

        # --- Vessels ---
        # Buyer 1 (Pacific Ocean Lines)
        db.add(Vessel(organization_id=buyer_orgs[0].id, name="Pacific Voyager", imo_number="9123456", vessel_type="Container Ship", flag_state="Singapore", dwt=150000))
        db.add(Vessel(organization_id=buyer_orgs[0].id, name="Pacific Pioneer", imo_number="9123457", vessel_type="Bulker", flag_state="Singapore", dwt=80000))
        
        # Buyer 2 (Atlantic Cargo)
        db.add(Vessel(organization_id=buyer_orgs[1].id, name="Atlantic Star", imo_number="9234567", vessel_type="Tanker", flag_state="USA", dwt=120000))
        db.add(Vessel(organization_id=buyer_orgs[1].id, name="Atlantic Spirit", imo_number="9234568", vessel_type="Container Ship", flag_state="USA", dwt=140000))

        # Buyer 3 (Nordic Shipping)
        db.add(Vessel(organization_id=buyer_orgs[2].id, name="Nordic Wind", imo_number="9345678", vessel_type="LPG Carrier", flag_state="Norway", dwt=60000))
        db.add(Vessel(organization_id=buyer_orgs[2].id, name="Nordic Ice", imo_number="9345679", vessel_type="LNG Carrier", flag_state="Norway", dwt=90000))

        # --- Inventory & Listings ---
        
        # Supplier 1 (Global Energy Supply) - Singapore & Rotterdam (Active)
        db.add(InventoryItem(supplier_id=supplier_orgs[0].id, port_id="SGSIN", fuel_type=FuelType.Biofuel, product_name="B24", current_stock_mt=5000, price_per_mt_usd=750))
        db.add(PublicListing(supplier_id=supplier_orgs[0].id, region="Singapore", fuel_type="Biofuel", fuel_grade=FuelGrade.BIO, quantity_mt=Decimal("5000"), price_per_mt_usd=Decimal("780"), availability_window=AvailabilityWindow.SPOT, tier_label=TierLabel.TIER_1_PRODUCER, is_verdaxis_verified=True))
        
        db.add(PublicListing(supplier_id=supplier_orgs[0].id, region="ARA", fuel_type="Methanol", fuel_grade=FuelGrade.GREEN, quantity_mt=Decimal("3000"), price_per_mt_usd=Decimal("550"), availability_window=AvailabilityWindow.Q1_2026, tier_label=TierLabel.TIER_1_PRODUCER, is_verdaxis_verified=True))

        # Supplier 2 (Eastern Bunkers) - Shanghai (Cheaper)
        db.add(InventoryItem(supplier_id=supplier_orgs[1].id, port_id="CNSHA", fuel_type=FuelType.LSMGO, product_name="LSMGO 0.1%", current_stock_mt=10000, price_per_mt_usd=600))
        db.add(PublicListing(supplier_id=supplier_orgs[1].id, region="Shanghai", fuel_type="LSMGO", fuel_grade=FuelGrade.CONVENTIONAL, quantity_mt=Decimal("10000"), price_per_mt_usd=Decimal("620"), availability_window=AvailabilityWindow.SPOT, tier_label=TierLabel.REGIONAL_SUPPLIER, is_verdaxis_verified=True))

        # Supplier 3 (Future Fuels Ltd) - Rotterdam (Green focus)
        db.add(InventoryItem(supplier_id=supplier_orgs[2].id, port_id="NLRTM", fuel_type=FuelType.LNG, product_name="Bio-LNG", current_stock_mt=2000, price_per_mt_usd=1200))
        db.add(PublicListing(supplier_id=supplier_orgs[2].id, region="ARA", fuel_type="LNG", fuel_grade=FuelGrade.BIO, quantity_mt=Decimal("2000"), price_per_mt_usd=Decimal("1250"), availability_window=AvailabilityWindow.SPOT, tier_label=TierLabel.INDEPENDENT, is_verdaxis_verified=True))

        db.add(PublicListing(supplier_id=supplier_orgs[2].id, region="Houston", fuel_type="Ammonia", fuel_grade=FuelGrade.GREEN, quantity_mt=Decimal("5000"), price_per_mt_usd=Decimal("900"), availability_window=AvailabilityWindow.FORWARD_2027, tier_label=TierLabel.INDEPENDENT, is_verdaxis_verified=False))

        await db.commit()
        logger.info("Seeding complete! Users created:")
        logger.info("  Admin: admin@verdaxis.com (password123)")
        
        for u in users:
            if u.role != UserRole.ADMIN:
                logger.info(f"  {u.role.value}: {u.email} (password123)")

if __name__ == "__main__":
    asyncio.run(seed())
