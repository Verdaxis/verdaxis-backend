# Model imports intentionally register every table on ``Base.metadata`` for
# migrations and portable test metadata; they are package side effects.
# ruff: noqa: F401

from app.models.user import User, Organization, OrganizationProvenance
from app.models.seed import MarketRowQuarantine, SeedRun
from app.models.market_event import MarketEventOutbox
from app.models.audit import AuditLog
from app.models.port import Port, PortIntelligence, Vessel
from app.models.marketplace import InventoryItem
from app.models.compliance import TraceabilityEvent, ComplianceLedger
from app.models.orders import Commission
from app.models.notification import Notification
from app.models.user_preference import UserPreference
from app.models.orderbook import OrderBookOrder, Trade
from app.models.matchmaking import MatchSuggestion
from app.models.producer import ProducerProject
from app.models.catalog import Product, DeliveryPoint
from app.models.subscription import Subscription, SubscriptionTier
from app.models.alerts import PriceAlert
from app.models.referral import Referral, ReferralStatus
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.models.watchlist import Watchlist, WatchlistEntry, WatchlistTarget, WatchlistEvent
from app.models.negotiation import Negotiation, NegotiationRound, NegotiationStatus
from app.models.news import NewsItem
from app.models.benchmark import Benchmark
from app.models.live_slice_benchmark import LiveSliceBenchmark
from app.models.forward_monitoring import FairPriceBand, MarketIndication, MarketSignalIngestionRun, PhysicalStem
from app.models.product_analytics import UserLoginDay, UserStatusTransition
from app.models.legacy import orders, direct_orders
from app.models.refresh_session import RefreshSession
from app.models.registration import PendingRegistration, OrganizationJoinRequest, JoinRequestStatus
