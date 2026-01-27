import asyncio
import logging
import uuid
from sqlalchemy import select
from app.database import AsyncSessionLocal
from app.models.user import User, UserRole, Organization, OrgType, UserStatus
from app.models.port import Port
from app.core.security import get_password_hash

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def seed():
    async with AsyncSessionLocal() as db:
        logger.info("Seeding data...")
        
        # Check if organizations exist
        result = await db.execute(select(Organization).limit(1))
        if result.scalar_one_or_none():
            logger.info("Data already exists, skipping seeding.")
            return

        # Create Organizations
        buyer_org = Organization(
            name="Maersk Line",
            type=OrgType.SHIPPING_LINE,
            country_code="DK"
        )
        supplier_org = Organization(
            name="Shell Marine",
            type=OrgType.FUEL_SUPPLIER,
            country_code="GB"
        )
        
        db.add(buyer_org)
        db.add(supplier_org)
        await db.flush() 

        # Create Users
        buyer_user = User(
            email="buyer@demo.com",
            first_name="Alice",
            last_name="Buyer",
            password_hash=get_password_hash("buyer123"),
            role=UserRole.BUYER,
            organization_id=buyer_org.id,
            status=UserStatus.APPROVED
        )

        supplier_user = User(
            email="supplier@demo.com",
            first_name="Bob",
            last_name="Supplier",
            password_hash=get_password_hash("supplier123"),
            role=UserRole.SUPPLIER,
            organization_id=supplier_org.id,
            status=UserStatus.APPROVED
        )

        admin_user = User(
            email="admin@verdaxis.com",
            first_name="Admin",
            last_name="Verdaxis",
            password_hash=get_password_hash("***REMOVED***"),
            role=UserRole.ADMIN,
            status=UserStatus.APPROVED
        )
        
        db.add(buyer_user)
        db.add(supplier_user)
        db.add(admin_user)
        
        # Seed some Ports
        rotterdam = Port(
            id="NLRTM",
            name="Rotterdam",
            country="Netherlands",
            timezone="Europe/Amsterdam",
            location="POINT(4.47917 51.9225)",
            is_active=True
        )
        singapore = Port(
            id="SGSIN",
            name="Singapore",
            country="Singapore",
            timezone="Asia/Singapore",
            location="POINT(103.8198 1.3521)",
            is_active=True
        )
        
        db.add(rotterdam)
        db.add(singapore)
        
        await db.commit()
        logger.info("Seeding complete!")
        logger.info(f"Buyer: buyer@demo.com / buyer123")
        logger.info(f"Supplier: supplier@demo.com / supplier123")
        logger.info(f"Admin: admin@verdaxis.com / ***REMOVED***")

if __name__ == "__main__":
    asyncio.run(seed())
