"""Email service using Resend REST API via httpx."""
import httpx
import structlog
import hashlib
import uuid
from html import escape
from typing import Any, Mapping
from app.config import settings

logger = structlog.get_logger()

RESEND_API_URL = "https://api.resend.com/emails"


def _recipient_hash(to_email: str) -> str:
    return hashlib.sha256(to_email.strip().lower().encode("utf-8")).hexdigest()[:16]


async def _send_email(
    to_email: str,
    subject: str,
    html: str,
    *,
    idempotency_key: str | None = None,
) -> bool:
    """Low-level send. Returns True on success, False on failure."""
    payload = {
        "from": settings.EMAIL_FROM,
        "to": [to_email],
        "subject": subject,
        "html": html,
    }
    return await _send_email_payload(payload, idempotency_key=idempotency_key)


def _canonical_email_payload(payload: Mapping[str, Any]) -> dict[str, Any] | None:
    """Validate and normalize a persisted payload into stable key order."""
    recipients = payload.get("to")
    if (
        not isinstance(payload.get("from"), str)
        or not isinstance(recipients, list)
        or len(recipients) != 1
        or not isinstance(recipients[0], str)
        or not isinstance(payload.get("subject"), str)
        or not isinstance(payload.get("html"), str)
    ):
        return None
    return {
        "from": payload["from"],
        "to": [recipients[0]],
        "subject": payload["subject"],
        "html": payload["html"],
    }


async def _send_email_payload(
    payload: Mapping[str, Any],
    *,
    idempotency_key: str | None = None,
) -> bool:
    """Send a validated payload without rebuilding mutable message fields."""
    canonical_payload = _canonical_email_payload(payload)
    if canonical_payload is None:
        logger.error("email_payload_invalid")
        return False
    recipient_hash = _recipient_hash(canonical_payload["to"][0])
    if not settings.RESEND_API_KEY:
        logger.warning("email_skipped", reason="configuration", recipient_hash=recipient_hash)
        return False

    headers = {"Authorization": f"Bearer {settings.RESEND_API_KEY}"}
    if idempotency_key is not None:
        headers["Idempotency-Key"] = idempotency_key
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.post(
                RESEND_API_URL,
                json=canonical_payload,
                headers=headers,
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


async def send_signup_alert_email(
    *, user_id: uuid.UUID, email: str, name: str, role: str, organization: str,
) -> bool:
    """Notify the admin mailbox after an application has been committed."""
    review_url = f"{settings.FRONTEND_URL.rstrip('/')}/app/admin/users"
    environment_label = "" if settings.ENVIRONMENT == "production" else f"[{settings.ENVIRONMENT}] "
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2>New Verdaxis account application</h2>
      <p>A new application has been submitted. Email verification and onboarding review may still be pending.</p>
      <dl>
        <dt>Name</dt><dd>{escape(name)}</dd>
        <dt>Email</dt><dd>{escape(email)}</dd>
        <dt>Role</dt><dd>{escape(role)}</dd>
        <dt>Organization</dt><dd>{escape(organization)}</dd>
      </dl>
      <p><a href="{escape(review_url, quote=True)}">Review applications in Verdaxis</a></p>
    </div>
    """
    return await _send_email(
        to_email="admin@verdaxis.exchange",
        subject=f"{environment_label}New Verdaxis account application",
        html=html,
        idempotency_key=f"signup-alert/{settings.ENVIRONMENT}/{user_id}",
    )


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


def build_account_approved_email_payload(to_email: str, name: str) -> dict[str, Any]:
    """Freeze the approval message at the time the approval is committed."""
    login_url = f"{settings.FRONTEND_URL.rstrip('/')}/login"
    html = f"""\
<div style="font-family: 'Segoe UI', Arial, sans-serif; max-width: 600px; margin: 0 auto; background: #0F172A; border-radius: 16px; overflow: hidden;">
  <div style="background: linear-gradient(135deg, #059669, #0F172A); padding: 32px 24px; text-align: center;">
    <h1 style="color: #fff; margin: 0; font-size: 24px; font-weight: 300;">Verdaxis</h1>
    <p style="color: rgba(255,255,255,0.5); margin: 6px 0 0; font-size: 13px;">Maritime Fuel Marketplace</p>
  </div>
  <div style="padding: 32px 24px;">
    <h2 style="color: #fff; font-size: 18px; margin: 0 0 12px;">Your account has been approved</h2>
    <p style="color: rgba(255,255,255,0.7); font-size: 14px; line-height: 1.6;">
      Hi {escape(name)},<br><br>
      Your Verdaxis account has been approved. You can now sign in and continue your onboarding.
    </p>
    <p style="margin: 28px 0; text-align: center;">
      <a href="{login_url}"
         style="display: inline-block; background: #10b981; color: #0F172A;
                padding: 14px 32px; text-decoration: none; border-radius: 10px;
                font-weight: 700; font-size: 15px;">
        Sign in to Verdaxis
      </a>
    </p>
    <p style="color: rgba(255,255,255,0.3); font-size: 11px; margin-top: 8px; word-break: break-all;">
      Or copy this link: <a href="{login_url}" style="color: #10b981;">{login_url}</a>
    </p>
  </div>
  <div style="border-top: 1px solid rgba(255,255,255,0.06); padding: 16px 24px; text-align: center;">
    <p style="color: rgba(255,255,255,0.2); font-size: 11px; margin: 0;">
      Verdaxis &middot; Maritime Fuel Marketplace
    </p>
  </div>
</div>"""
    return {
        "from": settings.EMAIL_FROM,
        "to": [to_email],
        "subject": "Your Verdaxis account has been approved",
        "html": html,
    }


async def send_account_approved_email(
    payload: Mapping[str, Any],
    transition_id: uuid.UUID,
) -> bool:
    """Replay one frozen approval message with transition-scoped deduplication."""
    return await _send_email_payload(
        payload,
        idempotency_key=f"account-approval/{transition_id}",
    )


async def send_kyc_approved_email(to_email: str, name: str) -> bool:
    """Send KYC approval notification."""
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">KYC Approved, {escape(name)}!</h2>
      <p>Your identity verification has been approved. You can continue with the remaining Verdaxis onboarding steps.</p>
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
