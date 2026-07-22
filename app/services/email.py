"""Email service using Resend REST API via httpx."""
import httpx
import structlog
import hashlib
from html import escape
from app.config import settings

logger = structlog.get_logger()

RESEND_API_URL = "https://api.resend.com/emails"


def _recipient_hash(to_email: str) -> str:
    return hashlib.sha256(to_email.strip().lower().encode("utf-8")).hexdigest()[:16]


async def _send_email(to_email: str, subject: str, html: str) -> bool:
    """Low-level send. Returns True on success, False on failure."""
    recipient_hash = _recipient_hash(to_email)
    if not settings.RESEND_API_KEY:
        logger.warning("email_skipped", reason="configuration", recipient_hash=recipient_hash)
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
            logger.info("email_sent", recipient_hash=recipient_hash, status=resp.status_code)
            return True
        else:
            # Provider response bodies can contain recipient/provider secrets.
            # Keep operational diagnostics bounded to status metadata.
            logger.error("email_send_failed", recipient_hash=recipient_hash, status=resp.status_code)
            return False
    except Exception as exc:
        logger.error(
            "email_send_error",
            recipient_hash=recipient_hash,
            exception_class=type(exc).__name__,
        )
        return False


async def send_verification_email(to_email: str, name: str, token: str) -> bool:
    """Send an email verification link."""
    verify_url = f"{settings.FRONTEND_URL}/verify-email?token={token}"
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">Welcome to Verdaxis, {escape(name)}!</h2>
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


async def send_password_reset_email(to_email: str, name: str, token: str) -> bool:
    """Send a password reset link with Verdaxis branding."""
    reset_url = f"{settings.FRONTEND_URL}/reset-password?token={token}"
    html = f"""\
<div style="font-family: 'Segoe UI', Arial, sans-serif; max-width: 600px; margin: 0 auto; background: #0F172A; border-radius: 16px; overflow: hidden;">
  <div style="background: linear-gradient(135deg, #059669, #0F172A); padding: 32px 24px; text-align: center;">
    <h1 style="color: #fff; margin: 0; font-size: 24px; font-weight: 300; letter-spacing: -0.02em;">Verdaxis</h1>
    <p style="color: rgba(255,255,255,0.5); margin: 6px 0 0; font-size: 13px;">Maritime Fuel Marketplace</p>
  </div>
  <div style="padding: 32px 24px;">
    <h2 style="color: #fff; font-size: 18px; margin: 0 0 12px;">Password Reset Request</h2>
    <p style="color: rgba(255,255,255,0.7); font-size: 14px; line-height: 1.6;">
      Hi {escape(name)},<br><br>
      We received a request to reset your password. Click the button below to set a new password.
      This link expires in <strong style="color: #10b981;">1 hour</strong>.
    </p>
    <p style="margin: 28px 0; text-align: center;">
      <a href="{reset_url}"
         style="display: inline-block; background: #10b981;
                color: #0F172A; padding: 14px 32px; text-decoration: none; border-radius: 10px;
                font-weight: 700; font-size: 15px; letter-spacing: 0.02em;">
        Reset My Password
      </a>
    </p>
    <p style="color: rgba(255,255,255,0.4); font-size: 12px; line-height: 1.5;">
      If you didn't request this, you can safely ignore this email. Your password will remain unchanged.
    </p>
    <p style="color: rgba(255,255,255,0.3); font-size: 11px; margin-top: 8px; word-break: break-all;">
      Or copy this link: <a href="{reset_url}" style="color: #10b981;">{reset_url}</a>
    </p>
  </div>
  <div style="border-top: 1px solid rgba(255,255,255,0.06); padding: 16px 24px; text-align: center;">
    <p style="color: rgba(255,255,255,0.2); font-size: 11px; margin: 0;">
      Verdaxis &middot; Maritime Fuel Marketplace
    </p>
  </div>
</div>"""
    return await _send_email(to_email, "Reset your Verdaxis password", html)


async def send_kyc_approved_email(to_email: str, name: str) -> bool:
    """Send KYC approval notification."""
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">KYC Approved, {escape(name)}!</h2>
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


async def send_referral_invite_email(to_email: str, referrer_name: str, referral_code: str) -> bool:
    """Send a referral invite email with Verdaxis branding."""
    invite_url = f"{settings.FRONTEND_URL}/invite/{referral_code}"
    referrer_display = escape(referrer_name) if referrer_name else "Someone"
    html = f"""\
<div style="font-family: 'Segoe UI', Arial, sans-serif; max-width: 600px; margin: 0 auto; background: #0F172A; border-radius: 16px; overflow: hidden;">
  <div style="background: linear-gradient(135deg, #059669, #0F172A); padding: 32px 24px; text-align: center;">
    <h1 style="color: #fff; margin: 0; font-size: 24px; font-weight: 300; letter-spacing: -0.02em;">Verdaxis</h1>
    <p style="color: rgba(255,255,255,0.5); margin: 6px 0 0; font-size: 13px;">Maritime Fuel Marketplace</p>
  </div>
  <div style="padding: 32px 24px;">
    <h2 style="color: #fff; font-size: 18px; margin: 0 0 12px;">You've been invited!</h2>
    <p style="color: rgba(255,255,255,0.7); font-size: 14px; line-height: 1.6;">
      {referrer_display} has invited you to join Verdaxis — the maritime fuel marketplace
      connecting buyers and suppliers worldwide.
    </p>
    <p style="margin: 28px 0; text-align: center;">
      <a href="{invite_url}"
         style="display: inline-block; background: #10b981;
                color: #0F172A; padding: 14px 32px; text-decoration: none; border-radius: 10px;
                font-weight: 700; font-size: 15px; letter-spacing: 0.02em;">
        Join Verdaxis
      </a>
    </p>
    <p style="color: rgba(255,255,255,0.3); font-size: 11px; margin-top: 8px; word-break: break-all;">
      Or copy this link: <a href="{invite_url}" style="color: #10b981;">{invite_url}</a>
    </p>
  </div>
  <div style="border-top: 1px solid rgba(255,255,255,0.06); padding: 16px 24px; text-align: center;">
    <p style="color: rgba(255,255,255,0.2); font-size: 11px; margin: 0;">
      Verdaxis &middot; Maritime Fuel Marketplace
    </p>
  </div>
</div>"""
    return await _send_email(to_email=to_email, subject=f"{referrer_display} invited you to Verdaxis", html=html)


async def send_kyc_rejected_email(to_email: str, name: str, reason: str) -> bool:
    """Send KYC rejection notification with reason."""
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">KYC Verification Update, {escape(name)}</h2>
      <p>We were unable to verify your identity documents. Your KYC submission has been rejected.</p>
      <div style="background: #fff3cd; border-left: 4px solid #ffc107; padding: 12px 16px; margin: 16px 0;">
        <strong>Reason:</strong> {escape(reason)}
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
