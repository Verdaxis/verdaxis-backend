import asyncio
import logging
import uuid
from sqlalchemy import select, text
from app.database import AsyncSessionLocal
from app.models.user import User, UserRole, Organization, OrgType, UserStatus
from app.models.port import Port, Vessel, CongestionLevel, PortIntelligence
from app.models.marketplace import InventoryItem, FuelType
from app.models.orders import PublicListing, FuelGrade, AvailabilityWindow, TierLabel, ListingStatus, Order, OrderStatus
from app.core.security import get_password_hash
from decimal import Decimal
from datetime import datetime, timedelta

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def seed():
    async with AsyncSessionLocal() as db:
        logger.info("Seeding expanded data...")
        
        # Clean existing data to ensure fresh seed
        logger.info("Cleaning existing data...")
        await db.execute(text("TRUNCATE TABLE inventory_items, orders, commissions, public_listings, vessels, users, organizations, ports, port_intelligence CASCADE"))
        await db.commit()
        
        logger.info("Creating organizations...")

        # --- Organizations ---
        # Buyers
        buyer_orgs = [
            Organization(name="Pacific Ocean Lines", type=OrgType.SHIPPING_LINE, country_code="SG", domain="pacificoceanlines.com"),
            Organization(name="Atlantic Cargo", type=OrgType.SHIPPING_LINE, country_code="US", domain="atlanticcargo.com"),
            Organization(name="Nordic Shipping", type=OrgType.SHIPPING_LINE, country_code="NO", domain="nordicshipping.no"),
            Organization(name="Maersk Green Ops", type=OrgType.SHIPPING_LINE, country_code="DK", domain="maersk.com"),
            Organization(name="Hapag-Lloyd Sustainability", type=OrgType.SHIPPING_LINE, country_code="DE", domain="hapag-lloyd.com")
        ]
        
        # Suppliers
        supplier_orgs = [
            Organization(name="Global Energy Supply", type=OrgType.FUEL_SUPPLIER, country_code="GB", domain="globalenergy.com"),
            Organization(name="Eastern Bunkers", type=OrgType.FUEL_SUPPLIER, country_code="CN", domain="easternbunkers.cn"),
            Organization(name="Future Fuels Ltd", type=OrgType.FUEL_SUPPLIER, country_code="NL", domain="futurefuels.nl"),
            Organization(name="Yara Clean Ammonia", type=OrgType.FUEL_SUPPLIER, country_code="NO", domain="yara.com"),
            Organization(name="Shell New Energies", type=OrgType.FUEL_SUPPLIER, country_code="NL", domain="shell.com")
        ]
        
        # Add Verdaxis Org for System Users
        verdaxis_org = Organization(name="Verdaxis Ltd", type=OrgType.FUEL_SUPPLIER, country_code="GB", domain="verdaxis.com")
        db.add(verdaxis_org)
        await db.flush()
        
        for o in buyer_orgs + supplier_orgs:
            db.add(o)
        await db.flush()

        logger.info("Creating users...")
        # --- Users ---
        users = []
        # Admin
        users.append(User(
            email="admin@verdaxis.com", first_name="Admin", last_name="User",
            password_hash=get_password_hash("admin123"), role=UserRole.ADMIN, status=UserStatus.APPROVED
        ))
        # Dev Admin (for backend bypass matching)
        users.append(User(
            email="dev@admin.com", first_name="Dev", last_name="Admin",
            password_hash=get_password_hash("admin123"), role=UserRole.ADMIN, status=UserStatus.APPROVED
        ))
        # Sarah Jenkins (Frontend Persona)
        users.append(User(
            email="sarah.jenkins@verdaxis.com", first_name="Sarah", last_name="Jenkins",
            password_hash=get_password_hash("password123"), role=UserRole.ADMIN, status=UserStatus.APPROVED
        ))

        # Standard Users for all Orgs
        for i, org in enumerate(buyer_orgs):
            u_id = f"00000000-0000-0000-0000-000000000b0{i+1}"
            users.append(User(
                id=uuid.UUID(u_id),
                email=f"buyer{i+1}@verdaxis.com", first_name=f"Buyer", last_name=str(i+1),
                password_hash=get_password_hash("password123"), role=UserRole.BUYER,
                organization_id=org.id, status=UserStatus.APPROVED
            ))
        
        for i, org in enumerate(supplier_orgs):
            u_id = f"00000000-0000-0000-0000-000000000a0{i+1}"
            users.append(User(
                id=uuid.UUID(u_id),
                email=f"supplier{i+1}@verdaxis.com", first_name="Supplier", last_name=str(i+1),
                password_hash=get_password_hash("password123"), role=UserRole.SUPPLIER,
                organization_id=org.id, status=UserStatus.APPROVED
            ))
            
        for u in users:
            db.add(u)
        
        logger.info("Creating ports and intelligence...")
        # --- Ports ---
        # coordinates: POINT(lng lat)
        port_data = [
            ("SGSIN", "Singapore", "Singapore", "Asia/Singapore", "POINT(103.8198 1.3521)", CongestionLevel.Moderate, 650, 780),
            ("NLRTM", "Rotterdam", "Netherlands", "Europe/Amsterdam", "POINT(4.47917 51.9225)", CongestionLevel.Low, 620, 750),
            ("USHOU", "Houston", "USA", "America/Chicago", "POINT(-95.3698 29.7604)", CongestionLevel.High, 580, 710),
            ("CNSHA", "Shanghai", "China", "Asia/Shanghai", "POINT(121.4737 31.2304)", CongestionLevel.Moderate, 600, 730),
            ("AEFUJ", "Fujairah", "UAE", "Asia/Dubai", "POINT(56.33 25.13)", CongestionLevel.Moderate, 630, 760),
            ("ESALG", "Algeciras", "Spain", "Europe/Madrid", "POINT(-5.45 36.13)", CongestionLevel.Low, 640, 770),
            ("PAPAN", "Panama City", "Panama", "America/Panama", "POINT(-79.52 8.98)", CongestionLevel.High, 670, 800),
            ("KRBUS", "Busan", "South Korea", "Asia/Seoul", "POINT(129.04 35.10)", CongestionLevel.Moderate, 610, 740),
            ("USLGB", "Long Beach", "USA", "America/Los_Angeles", "POINT(-118.19 33.77)", CongestionLevel.Moderate, 590, 720)
        ]
        
        ports = []
        for pid, name, country, tz, loc, cong, m_price, b_price in port_data:
            p = Port(id=pid, name=name, country=country, timezone=tz, location=loc, is_active=True)
            db.add(p)
            intel = PortIntelligence(
                port_id=pid, 
                congestion_level=cong, 
                methanol_price_avg=Decimal(str(m_price)), 
                biofuel_price_avg=Decimal(str(b_price))
            )
            db.add(intel)
            ports.append(p)
        await db.flush()

        logger.info("Creating vessels...")
        # --- Vessels ---
        vessel_names = [
            ("Pacific Voyager", "Container"), ("Pacific Pioneer", "Bulker"), ("Singapore Spirit", "Tanker"),
            ("Atlantic Star", "Tanker"), ("Atlantic Spirit", "Container"), ("Houston Express", "Bulker"),
            ("Nordic Wind", "LPG"), ("Nordic Ice", "LNG"), ("Oslo Pride", "Container"),
            ("Maersk Green 1", "Container"), ("Maersk Green 2", "Container"), ("Eco Voyager", "Bulker"),
            ("Hapaq Sustainability", "Container"), ("German Star", "Bulker"), ("Alster Dream", "Tanker")
        ]
        
        import math
        for i, (name, vtype) in enumerate(vessel_names):
            org_idx = i // 3
            # Use specific oceanic "safe points" to avoid landing on land
            import random
            oceanic_points = [
                (-30.0, 0.0),    # Mid-Atlantic
                (80.0, -20.0),   # Indian Ocean
                (115.0, 12.0),   # South China Sea
                (170.0, 40.0),   # North Pacific
                (-140.0, -30.0), # South Pacific
                (65.0, 15.0),    # Arabian Sea
                (90.0, 15.0),    # Bay of Bengal
                (18.0, 34.0),    # Central Mediterranean
                (0.0, 0.0),      # Gulf of Guinea
                (-20.0, 40.0),   # North Atlantic
                (105.0, -25.0)   # South Indian Ocean
            ]
            base_lng, base_lat = random.choice(oceanic_points)
            
            # Limited offset to stay within "safe" open waters
            curr_lng = base_lng + random.uniform(-3.0, 3.0)
            curr_lat = base_lat + random.uniform(-3.0, 3.0)
            
            # Previous location: offset to create a distinct heading
            angle = random.uniform(0, 2 * 3.14159)
            dist = random.uniform(0.5, 1.5)
            prev_lng = curr_lng - dist * math.sin(angle)
            prev_lat = curr_lat - dist * math.cos(angle)
            
            # No wrap needed for these safe mid-ocean points, but keep for safety
            curr_lng = (curr_lng + 180) % 360 - 180
            prev_lng = (prev_lng + 180) % 360 - 180

            db.add(Vessel(
                organization_id=buyer_orgs[org_idx].id,
                name=name,
                imo_number=f"9{i:06d}",
                vessel_type=vtype,
                flag_state=buyer_orgs[org_idx].country_code,
                dwt=Decimal(str(50000 + (i * 10000))),
                cii_rating="A" if i % 4 == 0 else "B" if i % 4 == 1 else "C",
                eu_ets_status="Compliant",
                fueleu_status="Compliant",
                current_location=f"POINT({curr_lng} {curr_lat})",
                previous_location=f"POINT({prev_lng} {prev_lat})",
                updated_at=datetime.utcnow()
            ))

        logger.info("Creating listings...")
        # --- Listings & Inventory ---
        listings_data = [
            (supplier_orgs[0], "Singapore", "Biofuel", FuelGrade.BIO, 5000, 780, AvailabilityWindow.SPOT),
            (supplier_orgs[0], "ARA", "Methanol", FuelGrade.GREEN, 3000, 550, AvailabilityWindow.Q1_2026),
            (supplier_orgs[1], "Shanghai", "LSMGO", FuelGrade.CONVENTIONAL, 10000, 620, AvailabilityWindow.SPOT),
            (supplier_orgs[1], "Busan", "LSMGO", FuelGrade.CONVENTIONAL, 8000, 615, AvailabilityWindow.SPOT),
            (supplier_orgs[2], "ARA", "LNG", FuelGrade.BIO, 2000, 1250, AvailabilityWindow.SPOT),
            (supplier_orgs[2], "Houston", "Ammonia", FuelGrade.GREEN, 5000, 900, AvailabilityWindow.FORWARD_2027),
            (supplier_orgs[3], "UAE", "Ammonia", FuelGrade.GREEN, 10000, 850, AvailabilityWindow.Q2_2026),
            (supplier_orgs[3], "Singapore", "Ammonia", FuelGrade.GREEN, 5000, 880, AvailabilityWindow.SPOT),
            (supplier_orgs[4], "ARA", "Methanol", FuelGrade.GREEN, 15000, 540, AvailabilityWindow.Q4_2025),
            (supplier_orgs[4], "Algeciras", "Biofuel", FuelGrade.BIO, 7000, 760, AvailabilityWindow.SPOT)
        ]
        
        created_listings = []
        for supplier, region, ftype, grade, qty, price, window in listings_data:
            listing = PublicListing(
                supplier_id=supplier.id,
                region=region,
                fuel_type=ftype,
                fuel_grade=grade,
                quantity_mt=Decimal(str(qty)),
                price_per_mt_usd=Decimal(str(price)),
                availability_window=window,
                tier_label=TierLabel.TIER_1_PRODUCER if qty > 8000 else TierLabel.MAJOR_TRADER,
                is_verdaxis_verified=True,
                status=ListingStatus.ACTIVE
            )
            db.add(listing)
            created_listings.append(listing)
        
        await db.flush() # Ensure listings have IDs

        logger.info("Creating historical matches...")
        # --- Historical Matches ---
        # Create some completed deals
        for i in range(5):
             db.add(Order(
                 listing_id=created_listings[i].id, 
                 buyer_id=buyer_orgs[i % 5].id,
                 status=OrderStatus.COMPLETED,
                 final_quantity_mt=Decimal("2500"),
                 final_price_per_mt=Decimal("560"),
                 final_total_usd=Decimal("1400000"),
                 completed_at=datetime.utcnow() - timedelta(days=i*10)
             ))

        await db.commit()
        logger.info("Seeding complete!")

if __name__ == "__main__":
    asyncio.run(seed())
