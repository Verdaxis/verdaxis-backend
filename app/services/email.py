"""Email service using Resend REST API via httpx."""
import httpx
import structlog
from app.config import settings

logger = structlog.get_logger()

RESEND_API_URL = "https://api.resend.com/emails"


async def _send_email(to_email: str, subject: str, html: str) -> bool:
    """Low-level send. Returns True on success, False on failure."""
    if not settings.RESEND_API_KEY:
        logger.warning("email_skipped", reason="RESEND_API_KEY not configured", to=to_email, subject=subject)
        return False

    payload = {
        "from": settings.EMAIL_FROM,
        "to": [to_email],
        "subject": subject,
        "html": html,
    }
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.post(
                RESEND_API_URL,
                json=payload,
                headers={"Authorization": f"Bearer {settings.RESEND_API_KEY}"},
            )
        if resp.status_code in (200, 201):
            logger.info("email_sent", to=to_email, subject=subject)
            return True
        else:
            logger.error("email_send_failed", to=to_email, status=resp.status_code, body=resp.text)
            return False
    except Exception as exc:
        logger.error("email_send_error", to=to_email, error=str(exc))
        return False


async def send_verification_email(to_email: str, name: str, token: str) -> bool:
    """Send an email verification link."""
    verify_url = f"{settings.FRONTEND_URL}/verify-email?token={token}"
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">Welcome to Verdaxis, {name}!</h2>
      <p>Please verify your email address to continue.</p>
      <p style="margin: 24px 0;">
        <a href="{verify_url}"
           style="background-color: #0066cc; color: white; padding: 12px 24px;
                  text-decoration: none; border-radius: 4px; font-weight: bold;">
          Verify Email Address
        </a>
      </p>
      <p style="color: #666; font-size: 14px;">
        Or copy this link: <a href="{verify_url}">{verify_url}</a>
      </p>
      <p style="color: #666; font-size: 14px;">This link expires in 24 hours.</p>
      <hr style="border: none; border-top: 1px solid #eee; margin: 24px 0;" />
      <p style="color: #999; font-size: 12px;">
        Verdaxis — Maritime Fuel Marketplace
      </p>
    </div>
    """
    return await _send_email(to_email, "Verify your Verdaxis email address", html)


async def send_kyc_approved_email(to_email: str, name: str) -> bool:
    """Send KYC approval notification."""
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">KYC Approved, {name}!</h2>
      <p>Your identity verification has been approved. Your Verdaxis account is now fully active.</p>
      <p style="margin: 24px 0;">
        <a href="{settings.FRONTEND_URL}/login"
           style="background-color: #00aa66; color: white; padding: 12px 24px;
                  text-decoration: none; border-radius: 4px; font-weight: bold;">
          Log in to Verdaxis
        </a>
      </p>
      <hr style="border: none; border-top: 1px solid #eee; margin: 24px 0;" />
      <p style="color: #999; font-size: 12px;">Verdaxis — Maritime Fuel Marketplace</p>
    </div>
    """
    return await _send_email(to_email, "Your Verdaxis KYC has been approved", html)


async def send_kyc_rejected_email(to_email: str, name: str, reason: str) -> bool:
    """Send KYC rejection notification with reason."""
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">KYC Verification Update, {name}</h2>
      <p>We were unable to verify your identity documents. Your KYC submission has been rejected.</p>
      <div style="background: #fff3cd; border-left: 4px solid #ffc107; padding: 12px 16px; margin: 16px 0;">
        <strong>Reason:</strong> {reason}
      </div>
      <p>Please resubmit your documents addressing the above issue.</p>
      <p style="margin: 24px 0;">
        <a href="{settings.FRONTEND_URL}/kyc"
           style="background-color: #0066cc; color: white; padding: 12px 24px;
                  text-decoration: none; border-radius: 4px; font-weight: bold;">
          Resubmit Documents
        </a>
      </p>
      <hr style="border: none; border-top: 1px solid #eee; margin: 24px 0;" />
      <p style="color: #999; font-size: 12px;">Verdaxis — Maritime Fuel Marketplace</p>
    </div>
    """
    return await _send_email(to_email, "Action required: Verdaxis KYC verification", html)
