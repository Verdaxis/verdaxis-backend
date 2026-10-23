# Model imports intentionally register every table on ``Base.metadata`` for
# migrations and portable test metadata; they are package side effects.
# ruff: noqa: F401

from app.models.alerts import PriceAlert
from app.models.audit import AuditLog
from app.models.benchmark import Benchmark
from app.models.catalog import DeliveryPoint, Product
from app.models.command_result import MarketCommandResult
from app.models.compliance import ComplianceLedger, TraceabilityEvent
from app.models.feedback import FeedbackEntry
from app.models.forward_monitoring import (
    FairPriceBand,
    MarketIndication,
    MarketSignalIngestionRun,
    PhysicalStem,
)
from app.models.kyc_document import KYCDocument
from app.models.kyc_execution_grace import KYCExecutionGrace, KYCGraceNotice
from app.models.legacy import direct_orders, orders
from app.models.live_slice_benchmark import LiveSliceBenchmark
from app.models.market_event import MarketEventOutbox
from app.models.market_support import (
    MarketSupportAuthorization,
    MarketSupportContext,
    StaffCapabilityAssignment,
)
from app.models.marketplace import InventoryItem
from app.models.matchmaking import MatchSuggestion
from app.models.negotiation import Negotiation, NegotiationRound, NegotiationStatus
from app.models.news import NewsItem
from app.models.notification import Notification
from app.models.orderbook import OrderBookOrder, Trade
from app.models.orders import Commission
from app.models.port import Port, PortIntelligence, Vessel
from app.models.producer import ProducerProject
from app.models.product_analytics import UserLoginDay, UserStatusTransition
from app.models.public_read_cache import (
    PublicReadCacheEvent,
    PublicReadCacheGeneration,
    PublicReadCacheIncarnation,
)
from app.models.referral import Referral, ReferralStatus
from app.models.refresh_session import RefreshSession
from app.models.registration import (
    JoinRequestStatus,
    OrganizationJoinRequest,
    PendingRegistration,
)
from app.models.rfq import RFQ, QuoteStatus, RFQQuote, RFQStatus
from app.models.seed import MarketRowQuarantine, OrganizationMarketApproval, SeedRun
from app.models.subscription import Subscription, SubscriptionTier
from app.models.supplier_offer import SupplierOffer
from app.models.user import Organization, OrganizationProvenance, User
from app.models.user_preference import UserPreference
from app.models.watchlist import (
    Watchlist,
    WatchlistEntry,
    WatchlistEvent,
    WatchlistTarget,
)
