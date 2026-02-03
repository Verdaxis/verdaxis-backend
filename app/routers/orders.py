from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, or_
from sqlalchemy.orm import selectinload, joinedload
from uuid import UUID
from datetime import datetime
from decimal import Decimal

from app.database import get_db
from app.models.orders import PublicListing, Order, Commission, ListingStatus, OrderStatus, CommissionStatus
from app.models.user import User, Organization, UserRole
from app.models.notification import Notification, NotificationType
from app.schemas.orders import (
    OrderCreate,
    OrderResponse,
    OrderDetailResponse,
    OrderUpdate,
    OrderComplete,
    CommissionResponse,
    CommissionSummary,
    CommissionUpdate,
)
from app.routers.auth_simple import get_current_user

router = APIRouter(prefix="/orders", tags=["orders"])


@router.post("", response_model=OrderResponse, status_code=status.HTTP_201_CREATED)
async def create_order(
    request_data: OrderCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Buyer requests a quote on an anonymized listing.
    """
    if current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can request quotes"
        )
    
    if not request_data.accepted_terms:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="You must accept the terms to proceed"
        )
    
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Buyer must belong to an organization"
        )
    
    # Find the listing
    result = await db.execute(select(PublicListing).where(
        PublicListing.id == request_data.listing_id,
        PublicListing.status == ListingStatus.ACTIVE
    ))
    listing = result.scalars().first()
    
    if not listing:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Listing not found or no longer active"
        )
    
    # Check if buyer already has a pending order for this listing
    result = await db.execute(select(Order).where(
        Order.listing_id == listing.id,
        Order.buyer_id == current_user.organization_id,
        Order.status == OrderStatus.PENDING
    ))
    existing_order = result.scalars().first()
    
    if existing_order:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="You already have a pending Order for this listing"
        )
    
    # Create the Order
    order = Order(
        listing_id=listing.id,
        buyer_id=current_user.organization_id,
        status=OrderStatus.PENDING,
        requested_quantity_mt=request_data.quantity_mt,
        requested_delivery_date=request_data.delivery_date,
        buyer_accepted_terms_at=datetime.utcnow(),
    )
    
    db.add(order)
    await db.flush() # Generate ID for notification
    
    # Notify supplier users
    # Fetch all users belonging to the supplier organization
    stmt = select(User).where(User.organization_id == listing.supplier_id)
    result = await db.execute(stmt)
    supplier_users = result.scalars().all()
    
    for user in supplier_users:
        notification = Notification(
            recipient_id=user.id,
            type=NotificationType.ORDER_UPDATE,
            title="New Order Request",
            message=f"A buyer has requested a quote for your {listing.fuel_type} listing in {listing.region}.",
            data={"order_id": str(order.id), "listing_id": str(listing.id)}
        )
        db.add(notification)

    await db.commit()
    await db.refresh(order)
    
    return order


@router.get("/my-requests", response_model=list[OrderDetailResponse])
async def list_buyer_orders(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all Orders made by the current buyer.
    """
    if current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can view their Orders"
        )
    
    # Eager load listing, listing.supplier, and buyer to avoid N+1 queries
    query = (
        select(Order)
        .options(
            joinedload(Order.listing).joinedload(PublicListing.supplier),
            joinedload(Order.buyer)
        )
        .where(Order.buyer_id == current_user.organization_id)
        .order_by(Order.created_at.desc())
    )
    
    result = await db.execute(query)
    orders = result.scalars().all()
    
    result_list = []
    for order in orders:
        listing = order.listing
        supplier = listing.supplier
        buyer = order.buyer
        
        result_list.append(OrderDetailResponse(
            id=order.id,
            listing_id=order.listing_id,
            buyer_id=order.buyer_id,
            status=order.status,
            buyer_accepted_terms_at=order.buyer_accepted_terms_at,
            created_at=order.created_at,
            region=listing.region,
            fuel_type=listing.fuel_type,
            fuel_grade=listing.fuel_grade,
            quantity_mt=listing.quantity_mt,
            price_per_mt_usd=listing.price_per_mt_usd,
            supplier_id=listing.supplier_id,
            supplier_name=supplier.name if supplier else "Unknown",
            buyer_name=buyer.name if buyer else "Unknown",
            requested_quantity_mt=order.requested_quantity_mt,
            requested_delivery_date=order.requested_delivery_date,
            final_quantity_mt=order.final_quantity_mt,
            final_price_per_mt=order.final_price_per_mt,
            final_total_usd=order.final_total_usd,
        ))
    
    return result_list


@router.get("/incoming", response_model=list[OrderDetailResponse])
async def list_supplier_incoming_orders(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all incoming Orders for the current supplier.
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can view incoming Orders"
        )
    
    # Get all listings owned by this supplier
    subquery = select(PublicListing.id).where(
        PublicListing.supplier_id == current_user.organization_id
    )
    
    # Eager load listing, listing.supplier (redundant but safe), and buyer
    query = (
        select(Order)
        .options(
            joinedload(Order.listing).joinedload(PublicListing.supplier),
            joinedload(Order.buyer)
        )
        .where(Order.listing_id.in_(subquery))
        .order_by(Order.created_at.desc())
    )
    
    result = await db.execute(query)
    orders = result.scalars().all()
    
    result_list = []
    for order in orders:
        listing = order.listing
        supplier = listing.supplier
        buyer = order.buyer
        
        result_list.append(OrderDetailResponse(
            id=order.id,
            listing_id=order.listing_id,
            buyer_id=order.buyer_id,
            status=order.status,
            buyer_accepted_terms_at=order.buyer_accepted_terms_at,
            created_at=order.created_at,
            region=listing.region,
            fuel_type=listing.fuel_type,
            fuel_grade=listing.fuel_grade,
            quantity_mt=listing.quantity_mt,
            price_per_mt_usd=listing.price_per_mt_usd,
            supplier_id=listing.supplier_id,
            supplier_name=supplier.name if supplier else "Unknown",
            buyer_name=buyer.name if buyer else "Unknown",
            requested_quantity_mt=order.requested_quantity_mt,
            requested_delivery_date=order.requested_delivery_date,
            final_quantity_mt=order.final_quantity_mt,
            final_price_per_mt=order.final_price_per_mt,
            final_total_usd=order.final_total_usd,
        ))
    
    return result_list


@router.put("/{order_id}/respond", response_model=OrderResponse)
async def respond_to_order(
    order_id: UUID,
    response_data: OrderUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Supplier responds to an Order (accept or decline).
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can respond to Orders"
        )
    
    # Eager load listing to check ownership
    result = await db.execute(select(Order).options(selectinload(Order.listing)).where(Order.id == order_id))
    order = result.scalars().first()
    
    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found"
        )
    
    # Verify supplier owns the listing
    if order.listing.supplier_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only respond to Orders for your own listings"
        )
    
    if order.status != OrderStatus.PENDING:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"Cannot respond to Order with status: {order.status}"
        )
    
    order.status = response_data.status
    order.supplier_responded_at = datetime.utcnow()

    # Notify buyer users
    stmt = select(User).where(User.organization_id == order.buyer_id)
    result = await db.execute(stmt)
    buyer_users = result.scalars().all()
    
    status_msg = "accepted" if response_data.status == OrderStatus.ACCEPTED else "declined"

    # Inventory Reservation on Acceptance
    if response_data.status == OrderStatus.ACCEPTED:
        # Check if enough stock
        if order.listing.quantity_mt < order.requested_quantity_mt:
             raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail=f"Insufficient stock to accept this order. Requested: {order.requested_quantity_mt}, Available: {order.listing.quantity_mt}"
            )
        
        # Deduct / Reserve stock
        # Ensure we are working with Decimals
        order.listing.quantity_mt -= order.requested_quantity_mt
        
        # Check depletion
        if order.listing.quantity_mt <= 0:
            order.listing.quantity_mt = Decimal(0)
            order.listing.status = ListingStatus.INACTIVE
            
        # AUTO-CANCEL: Check other pending orders for this listing
        # If any pending order requests more than the *now remaining* quantity, decline it.
        stmt_pending = select(Order).where(
            Order.listing_id == order.listing_id,
            Order.status == OrderStatus.PENDING,
            Order.id != order.id # Exclude current order
        )
        result_pending = await db.execute(stmt_pending)
        pending_orders = result_pending.scalars().all()
        
        for pending in pending_orders:
            if pending.requested_quantity_mt > order.listing.quantity_mt:
                # Auto-decline
                pending.status = OrderStatus.DECLINED
                
                # Notify buyer
                stmt_buyer_users = select(User).where(User.organization_id == pending.buyer_id)
                res_u = await db.execute(stmt_buyer_users)
                p_buyers = res_u.scalars().all()
                for p_user in p_buyers:
                     db.add(Notification(
                        recipient_id=p_user.id,
                        type=NotificationType.ORDER_UPDATE,
                        title="Order Auto-Declined",
                        message=f"Your order for {pending.requested_quantity_mt} MT was declined because the available stock has dropped to {order.listing.quantity_mt} MT.",
                        data={"order_id": str(pending.id)}
                    ))
    
    for user in buyer_users:
        notification = Notification(
            recipient_id=user.id,
            type=NotificationType.ORDER_UPDATE,
            title=f"Order {status_msg.capitalize()}",
            message=f"The supplier has {status_msg} your Order.",
            data={"order_id": str(order.id)}
        )
        db.add(notification)
    
    await db.commit()
    await db.refresh(order)
    
    return order


@router.put("/{order_id}/complete", response_model=OrderResponse)
async def complete_order(
    order_id: UUID,
    completion_data: OrderComplete,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Complete an Order match and calculate commission.
    """
    # Eager load listing to check ownership
    result = await db.execute(select(Order).options(selectinload(Order.listing)).where(Order.id == order_id))
    order = result.scalars().first()
    
    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found"
        )
    
    # Can be completed by buyer or supplier
    is_buyer = order.buyer_id == current_user.organization_id
    is_supplier = order.listing.supplier_id == current_user.organization_id
    
    if not (is_buyer or is_supplier):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only matched parties can complete this Order"
        )
    
    if order.status != OrderStatus.ACCEPTED:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Order must be ACCEPTED before completion"
        )
    
    # Set final deal details
    order.final_quantity_mt = completion_data.final_quantity_mt
    order.final_price_per_mt = completion_data.final_price_per_mt
    order.final_total_usd = completion_data.final_quantity_mt * completion_data.final_price_per_mt
    order.completed_at = datetime.utcnow()
    order.status = OrderStatus.COMPLETED
    
    # Calculate commission
    commission_amount = order.final_total_usd * (order.commission_rate_pct / 100)
    order.commission_amount_usd = commission_amount
    
    # Create commission record
    commission = Commission(
        match_id=order.id,
        amount_usd=commission_amount,
        status=CommissionStatus.PENDING,
    )
    
    db.add(commission)

    # Update Listing Inventory
    listing = order.listing
    
    # Inventory was already reserved (deducted) at Acceptance based on requested_quantity_mt.
    # Now valid adjust for any difference in the final quantity.
    # e.g., Requested 50, Final 45 -> Return 5 to inventory.
    # e.g., Requested 50, Final 55 -> Deduct 5 more (if available).
    
    quantity_diff = order.requested_quantity_mt - order.final_quantity_mt
    
    if quantity_diff != 0:
        listing.quantity_mt += quantity_diff
        
    # Check if inventory is depleted (or negative, which shouldn't happen but good to handle)
    if listing.quantity_mt <= 0:
        listing.quantity_mt = Decimal(0)
        listing.status = ListingStatus.INACTIVE
    else:
        # If it was inactive but we refunded stock, we might want to make it active again?
        # For now, let's keep it simple. If we add stock back, we ensure it's ACTIVE if > 0.
        if listing.status == ListingStatus.INACTIVE and listing.quantity_mt > 0:
            listing.status = ListingStatus.ACTIVE
        
    await db.commit()
    await db.refresh(order)
    
    return order


# ============== Admin Commission Endpoints ==============

@router.get("/admin/commissions", response_model=list[CommissionResponse])
async def list_all_commissions(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    List all commissions (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    result = await db.execute(select(Commission).order_by(Commission.created_at.desc()))
    commissions = result.scalars().all()
    return commissions


@router.get("/admin/commissions/summary", response_model=CommissionSummary)
async def get_commission_summary(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get commission summary stats (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    from sqlalchemy import func
    
    query_pending = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.PENDING)
    res_pending = await db.execute(query_pending)
    pending = res_pending.one()
    
    query_invoiced = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.INVOICED)
    res_invoiced = await db.execute(query_invoiced)
    invoiced = res_invoiced.one()
    
    query_paid = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.PAID)
    res_paid = await db.execute(query_paid)
    paid = res_paid.one()
    
    return CommissionSummary(
        pending_count=pending[0],
        total_pending_usd=pending[1],
        invoiced_count=invoiced[0],
        total_invoiced_usd=invoiced[1],
        paid_count=paid[0],
        total_paid_usd=paid[1],
    )


@router.put("/admin/commissions/{commission_id}", response_model=CommissionResponse)
async def update_commission(
    commission_id: UUID,
    update_data: CommissionUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Update commission status (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    result = await db.execute(select(Commission).where(Commission.id == commission_id))
    commission = result.scalars().first()
    
    if not commission:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Commission not found"
        )
    
    update_dict = update_data.model_dump(exclude_unset=True)
    for field, value in update_dict.items():
        setattr(commission, field, value)
    
    await db.commit()
    await db.refresh(commission)
    
    return commission
