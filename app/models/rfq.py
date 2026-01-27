from sqlalchemy import String, ForeignKey, Enum, Numeric, Date, DateTime, Boolean, JSON
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, date
from decimal import Decimal
from app.database import Base


class ListingStatus(str, enum.Enum):
    ACTIVE = "ACTIVE"
    INACTIVE = "INACTIVE"
    EXPIRED = "EXPIRED"


class MatchStatus(str, enum.Enum):
    PENDING = "PENDING"       # Buyer sent RFQ, awaiting supplier response
    ACCEPTED = "ACCEPTED"     # Supplier accepted, negotiation can begin
    DECLINED = "DECLINED"     # Supplier declined
    COMPLETED = "COMPLETED"   # Deal completed, commission due
    CANCELLED = "CANCELLED"   # Buyer cancelled


class CommissionStatus(str, enum.Enum):
    PENDING = "PENDING"       # Match completed, commission calculated
    INVOICED = "INVOICED"     # Invoice sent to parties
    PAID = "PAID"             # Payment received


class FuelGrade(str, enum.Enum):
    CONVENTIONAL = "Conventional"
    GREEN = "Green"
    BIO = "Bio"


class AvailabilityWindow(str, enum.Enum):
    SPOT = "Spot"
    Q1_2025 = "Q1 2025"
    Q2_2025 = "Q2 2025"
    Q3_2025 = "Q3 2025"
    Q4_2025 = "Q4 2025"
    Q1_2026 = "Q1 2026"
    Q2_2026 = "Q2 2026"
    Q3_2026 = "Q3 2026"
    Q4_2026 = "Q4 2026"
    FORWARD_2027 = "Forward 2027"
    FORWARD_2028 = "Forward 2028"


class TierLabel(str, enum.Enum):
    TIER_1_PRODUCER = "Tier 1 Producer"
    MAJOR_TRADER = "Major Trader"
    REGIONAL_SUPPLIER = "Regional Supplier"
    INDEPENDENT = "Independent Supplier"


class PublicListing(Base):
    """
    Anonymized fuel listing visible to all buyers.
    Supplier identity is hidden until RFQ match is confirmed.
    """
    __tablename__ = "public_listings"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    
    # Hidden from buyers - only revealed after RFQ match
    supplier_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    
    # Public fields
    region: Mapped[str] = mapped_column(String(50), nullable=False)  # "Singapore", "ARA", "Houston"
    fuel_type: Mapped[str] = mapped_column(String(50), nullable=False)  # Uses marketplace FuelType
    fuel_grade: Mapped[FuelGrade] = mapped_column(Enum(FuelGrade, native_enum=False), default=FuelGrade.CONVENTIONAL)
    
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    
    availability_window: Mapped[AvailabilityWindow] = mapped_column(
        Enum(AvailabilityWindow, native_enum=False), 
        default=AvailabilityWindow.SPOT
    )
    
    # Anonymized label shown to buyers
    tier_label: Mapped[TierLabel] = mapped_column(
        Enum(TierLabel, native_enum=False), 
        default=TierLabel.REGIONAL_SUPPLIER
    )
    
    # Certifications as JSON array: ["ISCC", "Nanolumi", "ProofOfSustainability"]
    certifications: Mapped[list | None] = mapped_column(JSON, default=list)
    is_verdaxis_verified: Mapped[bool] = mapped_column(Boolean, default=False)
    
    # Status
    status: Mapped[ListingStatus] = mapped_column(
        Enum(ListingStatus, native_enum=False), 
        default=ListingStatus.ACTIVE
    )
    
    # Timestamps
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    
    # Relationships
    supplier = relationship("Organization", back_populates="listings")
    matches = relationship("RFQMatch", back_populates="listing")


class RFQMatch(Base):
    """
    Created when a buyer requests a quote on an anonymized listing.
    This de-anonymizes the parties and tracks the match lifecycle.
    """
    __tablename__ = "rfq_matches"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    
    # Links
    listing_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("public_listings.id"), nullable=False)
    buyer_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    
    # Match lifecycle
    status: Mapped[MatchStatus] = mapped_column(
        Enum(MatchStatus, native_enum=False), 
        default=MatchStatus.PENDING
    )
    
    # Timestamps
    buyer_accepted_terms_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    supplier_notified_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    supplier_responded_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    completed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    
    # Final deal details (populated on completion)
    final_quantity_mt: Mapped[Decimal | None] = mapped_column(Numeric(12, 2))
    final_price_per_mt: Mapped[Decimal | None] = mapped_column(Numeric(10, 2))
    final_total_usd: Mapped[Decimal | None] = mapped_column(Numeric(14, 2))
    
    # Commission
    commission_rate_pct: Mapped[Decimal] = mapped_column(Numeric(5, 3), default=Decimal("0.5"))  # 0.5% default
    commission_amount_usd: Mapped[Decimal | None] = mapped_column(Numeric(12, 2))
    
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    
    # Relationships
    listing = relationship("PublicListing", back_populates="matches")
    buyer = relationship("Organization", foreign_keys=[buyer_id])
    commission = relationship("Commission", back_populates="match", uselist=False)


class Commission(Base):
    """
    Tracks commission owed to Verdaxis from completed RFQ matches.
    Used by Admin dashboard to monitor revenue.
    """
    __tablename__ = "commissions"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    
    match_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("rfq_matches.id"), unique=True, nullable=False)
    
    # Financials
    amount_usd: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    status: Mapped[CommissionStatus] = mapped_column(
        Enum(CommissionStatus, native_enum=False), 
        default=CommissionStatus.PENDING
    )
    
    # Invoice tracking
    invoice_number: Mapped[str | None] = mapped_column(String(50))
    invoice_date: Mapped[date | None] = mapped_column(Date)
    payment_date: Mapped[date | None] = mapped_column(Date)
    
    notes: Mapped[str | None] = mapped_column(String(500))
    
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)
    
    # Relationships
    match = relationship("RFQMatch", back_populates="commission")
