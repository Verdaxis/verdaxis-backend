"""Email service using Resend REST API via httpx."""
import httpx
import structlog
import hashlib
import uuid
from html import escape
from typing import Any, Mapping
from urllib.parse import quote
from app.config import settings

logger = structlog.get_logger()

RESEND_API_URL = "https://api.resend.com/emails"
SUPPORT_EMAIL = "admin@verdaxis.exchange"


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
        "reply_to": SUPPORT_EMAIL,
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
    canonical_payload = {
        "from": payload["from"],
        "to": [recipients[0]],
        "subject": payload["subject"],
        "html": payload["html"],
    }
    # Old approval messages must replay without adding fields under their
    # existing idempotency key. New messages freeze the reply destination.
    if "reply_to" in payload:
        if not isinstance(payload["reply_to"], str):
            return None
        canonical_payload["reply_to"] = payload["reply_to"]
    return canonical_payload


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


def _render_email(
    *,
    title: str,
    preview: str,
    label: str,
    body_html: str,
    action_label: str,
    action_url: str,
    note: str = "",
) -> str:
    """Render trusted body HTML; callers must escape all applicant content."""
    safe_url = escape(action_url, quote=True)
    # Match the app's light palette. Inline styles and tables also work without
    # media-query support; the public PNG is compatible with email image proxies.
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="light">
  <meta name="supported-color-schemes" content="light">
  <title>{escape(title)}</title>
  <style>
    body {{ margin: 0; padding: 0; }}
    table {{ border-spacing: 0; }}
    @media only screen and (max-width: 620px) {{
      .email-outer {{ padding: 20px 12px !important; }}
      .email-content {{ padding: 28px 24px !important; }}
      .email-title {{ font-size: 26px !important; line-height: 34px !important; }}
      .detail-label {{ display: block !important; width: auto !important; padding: 12px 16px 0 !important; }}
      .detail-value {{ display: block !important; padding: 0 16px 12px !important; }}
    }}
  </style>
</head>
<body bgcolor="#F8FAFC" style="margin:0;padding:0;background-color:#F8FAFC;color:#334155;font-family:'Segoe UI',Arial,sans-serif;-webkit-text-size-adjust:100%;color-scheme:light;">
  <div style="display:none;font-size:1px;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden;mso-hide:all;">{escape(preview)}</div>
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="#F8FAFC">
    <tr><td class="email-outer" align="center" style="padding:40px 16px;">
      <!--[if mso]><table role="presentation" width="600" align="center"><tr><td><![endif]-->
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="#FFFFFF" style="width:100%;max-width:600px;background-color:#FFFFFF;border:1px solid #E2E8F0;border-top:3px solid #24558A;border-radius:16px;">
        <tr><td class="email-content" style="padding:32px 40px 28px;border-bottom:1px solid #E2E8F0;">
          <img src="https://app.verdaxis.exchange/verdaxis-logo-words-right.png" width="216" height="49" alt="Verdaxis" style="display:block;width:216px;max-width:100%;height:auto;border:0;color:#24558A;font-size:24px;font-weight:700;">
          <p style="margin:16px 0 0;color:#64748B;font-size:11px;line-height:18px;font-weight:600;letter-spacing:1.5px;">LOW-CARBON FUELS EXCHANGE</p>
        </td></tr>
        <tr><td class="email-content" style="padding:36px 40px 40px;color:#334155;font-size:16px;line-height:26px;overflow-wrap:anywhere;word-wrap:break-word;">
          <p style="margin:0 0 10px;color:#24558A;font-size:12px;line-height:18px;font-weight:700;letter-spacing:1.2px;">{escape(label)}</p>
          <h1 class="email-title" style="margin:0 0 24px;color:#0F172A;font-size:30px;line-height:38px;letter-spacing:-0.6px;font-weight:700;">{escape(title)}</h1>
          {body_html}
          <table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:28px 0 0;">
            <tr><td bgcolor="#5DADE2" style="background-color:#5DADE2;border-radius:8px;text-align:center;mso-padding-alt:14px 24px;">
              <a href="{safe_url}" style="display:inline-block;border:1px solid #5DADE2;border-radius:8px;padding:13px 23px;color:#0F172A;font-size:15px;line-height:22px;font-weight:700;text-decoration:none;mso-padding-alt:0;">{escape(action_label)}</a>
            </td></tr>
          </table>
          {f'<p style="margin:20px 0 0;color:#64748B;font-size:14px;line-height:22px;">{escape(note)}</p>' if note else ''}
          <p style="margin:28px 0 0;padding-top:20px;border-top:1px solid #E2E8F0;color:#64748B;font-size:12px;line-height:20px;">
            If the button does not work, copy this link:<br>
            <a href="{safe_url}" style="color:#24558A;text-decoration:underline;word-break:break-all;">{safe_url}</a>
          </p>
        </td></tr>
      </table>
      <!--[if mso]></td></tr></table><![endif]-->
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:600px;">
        <tr><td align="center" style="padding:24px 16px 0;color:#64748B;font-size:12px;line-height:20px;">
          <strong style="color:#334155;font-weight:600;">Verdaxis</strong> &nbsp;&middot;&nbsp; Low-carbon fuels exchange<br>
          <p style="margin:10px 0 0;font-size:14px;line-height:22px;">Need a hand? Reply to this email or<br>
            <a href="mailto:{SUPPORT_EMAIL}" style="color:#24558A;text-decoration:underline;font-weight:600;">contact the Verdaxis team</a>.
          </p>
        </td></tr>
      </table>
    </td></tr>
  </table>
</body>
</html>"""


async def send_verification_email(to_email: str, name: str, token: str) -> bool:
    """Send an email verification link."""
    html = _render_email(
        title="Verify your email address.",
        preview="One quick step to continue your Verdaxis application.",
        label="GETTING STARTED",
        body_html=f"""<p style="margin:0 0 16px;">Hi {escape(name)},</p>
          <p style="margin:0;">Welcome to Verdaxis. Please verify your email address to continue your application.</p>""",
        action_label="Verify email address",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/verify-email?token={quote(token, safe='')}",
        note="This link expires in 24 hours. If you did not create an account, you can ignore this email.",
    )
    return await _send_email(to_email, "Verify your Verdaxis email address", html)


async def send_signup_alert_email(
    *, user_id: uuid.UUID, email: str, name: str, role: str, organization: str,
) -> bool:
    """Notify the admin mailbox after an application has been committed."""
    environment_label = "" if settings.ENVIRONMENT == "production" else f"[{settings.ENVIRONMENT}] "
    details = "".join(
        f'<tr><th class="detail-label" scope="row" align="left" valign="top" style="width:100px;padding:10px 16px;color:#64748B;font-size:13px;line-height:22px;font-weight:400;">{field}</th>'
        f'<td class="detail-value" valign="top" style="padding:10px 16px 10px 0;color:#334155;font-size:14px;line-height:22px;font-weight:600;word-break:break-word;">{escape(value)}</td></tr>'
        for field, value in (("Name", name), ("Email", email), ("Role", role), ("Organization", organization))
    )
    html = _render_email(
        title="A new application is in.",
        preview=f"{name} from {organization} has applied to join Verdaxis.",
        label=f"{environment_label}ADMIN REVIEW",
        body_html=f"""<p style="margin:0 0 24px;">A new applicant has joined the review queue. Their details are below.</p>
          <table width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="#F8FAFC" aria-label="Applicant details" style="width:100%;table-layout:fixed;background-color:#F8FAFC;border:1px solid #E2E8F0;border-radius:8px;">
            {details}
          </table>""",
        action_label="Review application",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/app/admin/users",
        note="Email verification and onboarding review may still be pending.",
    )
    return await _send_email(
        to_email=SUPPORT_EMAIL,
        subject=f"{environment_label}New Verdaxis account application",
        html=html,
        idempotency_key=f"signup-alert/{settings.ENVIRONMENT}/{user_id}",
    )


async def send_password_reset_email(to_email: str, name: str, token: str) -> bool:
    """Send a password reset link with Verdaxis branding."""
    html = _render_email(
        title="Reset your password.",
        preview="Your secure password reset link is valid for one hour.",
        label="ACCOUNT SECURITY",
        body_html=f"""<p style="margin:0 0 16px;">Hi {escape(name)},</p>
          <p style="margin:0;">We received a request to reset your Verdaxis password. Use the link below to choose a new one.</p>""",
        action_label="Reset password",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/reset-password?token={quote(token, safe='')}",
        note="This link expires in 1 hour. If you did not request a reset, you can ignore this email. Your password will remain unchanged.",
    )
    return await _send_email(to_email, "Reset your Verdaxis password", html)


def build_account_approved_email_payload(to_email: str, name: str) -> dict[str, Any]:
    """Freeze the approval message at the time the approval is committed."""
    html = _render_email(
        title="Welcome to Verdaxis.",
        preview="Your account has been approved. Your next step starts here.",
        label="ACCOUNT APPROVED",
        body_html=f"""<p style="margin:0 0 16px;">Congratulations, {escape(name)}. Your account has been approved.</p>
          <p style="margin:0;">Complete any remaining onboarding steps, then explore the marketplace and start your next low-carbon fuel trade.</p>""",
        action_label="Explore the marketplace",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/app/marketplace",
    )
    return {
        "from": settings.EMAIL_FROM,
        "reply_to": SUPPORT_EMAIL,
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
    html = _render_email(
        title="Identity verification complete.",
        preview="Your identity documents have been approved.",
        label="VERIFICATION APPROVED",
        body_html=f"""<p style="margin:0 0 16px;">Hi {escape(name)},</p>
          <p style="margin:0;">Your identity verification has been approved. You can continue with the remaining Verdaxis onboarding steps.</p>""",
        action_label="Continue to Verdaxis",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/login",
    )
    return await _send_email(to_email, "Your Verdaxis KYC has been approved", html)


async def send_referral_invite_email(to_email: str, referrer_name: str, referral_code: str) -> bool:
    """Send a referral invite email with Verdaxis branding."""
    referrer_display = referrer_name or "Someone"
    html = _render_email(
        title="You are invited to Verdaxis.",
        preview=f"{referrer_display} has invited you to join the low-carbon fuels exchange.",
        label="YOUR INVITATION",
        body_html=f"""<p style="margin:0 0 16px;">{escape(referrer_display)} has invited you to join Verdaxis.</p>
          <p style="margin:0;">Connect with buyers and suppliers, explore the market, and take your next step in low-carbon fuels.</p>""",
        action_label="Explore your invitation",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/invite/{quote(referral_code, safe='')}",
    )
    return await _send_email(
        to_email=to_email,
        subject=f"{referrer_display} invited you to Verdaxis",
        html=html,
    )


async def send_kyc_rejected_email(to_email: str, name: str, reason: str) -> bool:
    """Send KYC rejection notification with reason."""
    html = _render_email(
        title="Your documents need an update.",
        preview="Please review the feedback and resubmit your identity documents.",
        label="ACTION REQUIRED",
        body_html=f"""<p style="margin:0 0 16px;">Hi {escape(name)},</p>
          <p style="margin:0 0 20px;">We could not approve your identity documents. Please address the feedback below and resubmit them.</p>
          <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" bgcolor="#FFFBEB" style="background-color:#FFFBEB;border-left:3px solid #D97706;">
            <tr><td style="padding:16px 20px;color:#92400E;font-size:14px;line-height:22px;overflow-wrap:anywhere;">
              <strong>Review feedback</strong><br>{escape(reason)}
            </td></tr>
          </table>""",
        action_label="Update your documents",
        action_url=f"{settings.FRONTEND_URL.rstrip('/')}/kyc",
    )
    return await _send_email(to_email, "Action required: Verdaxis KYC verification", html)
