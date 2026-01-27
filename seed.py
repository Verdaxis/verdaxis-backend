import asyncio
import logging
from app.database import async_session_factory
from app.models.user import User, UserRole, Organization, OrganizationType
from app.core.security import get_password_hash

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def seed():
    async with async_session_factory() as db:
        logger.info("Seeding data...")
        
        # Check if data exists
        # ... logic to avoid duplicates ...
        
        # Create Organizations
        buyer_org = Organization(
            name="Maersk Line",
            type=OrganizationType.BUYER,
            domain="maersk.com"
        )
        supplier_org = Organization(
            name="Shell Marine",
            type=OrganizationType.SUPPLIER,
            domain="shell.com"
        )
        
        db.add(buyer_org)
        db.add(supplier_org)
        await db.flush() # get IDs

        # Create Users
        buyer_user = User(
            email="buyer@demo.com",
            full_name="Alice Buyer",
            password_hash=get_password_hash("buyer123"),
            role=UserRole.BUYER,
            organization_id=buyer_org.id
        )

        supplier_user = User(
            email="supplier@demo.com",
            full_name="Bob Supplier",
            password_hash=get_password_hash("supplier123"),
            role=UserRole.SUPPLIER,
            organization_id=supplier_org.id
        )
        
        db.add(buyer_user)
        db.add(supplier_user)
        
        await db.commit()
        logger.info("Seeding complete!")
        logger.info(f"Buyer: buyer@demo.com / buyer123")
        logger.info(f"Supplier: supplier@demo.com / supplier123")

if __name__ == "__main__":
    asyncio.run(seed())
