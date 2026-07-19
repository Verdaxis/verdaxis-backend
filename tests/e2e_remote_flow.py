import httpx
import os
import time
import uuid
import sys
import json
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

# Configuration
from tests.runtime_config import resolve_test_api_url

BASE_URL = resolve_test_api_url(os.environ, require_mutation_opt_in=True) + "/api"
TIMEOUT = 30

def log(msg):
    print(f"[E2E] {msg}")

def log_resp(resp):
    try:
        data = resp.json()
        print(f"      Response ({resp.status_code}): {json.dumps(data, indent=2)}")
    except:
        print(f"      Response ({resp.status_code}): {resp.text}")

def generate_email(prefix, domain="test.com"):
    return f"{prefix}_{uuid.uuid4().hex[:8]}@{domain}"

def get_auth_headers(token):
    return {"Authorization": f"Bearer {token}"}

def register_user(client, email, role, org_name, org_type):
    log(f"[{role}] Registering {email}...")
    
    # 1. Register User
    reg_payload = {
        "email": email,
        "password": "password123",
        "first_name": "Test",
        "last_name": "User",
        "role": role
    }
    log(f"   POST /auth/register payload: {reg_payload}")
    resp = client.post(f"{BASE_URL}/auth/register", json=reg_payload)
    if resp.status_code != 200:
        log(f"Registration failed: {resp.text}")
        sys.exit(1)
        
    data = resp.json()
    log(f"   Registration status: {data.get('status')}")
    
    if data["status"] == "requires_org":
        log(f"[{role}] Creating Organization {org_name}...")
        token = data["registration_token"]
        
        # 2. Create Organization
        org_payload = {
            "registration_token": token,
            "organization": {
                "name": org_name,
                "type": org_type,
                "country_code": "NL",
                "tax_id": "12345"
            }
        }
        resp = client.post(f"{BASE_URL}/auth/register-with-org", json=org_payload)
        if resp.status_code != 200:
            log(f"Org creation failed: {resp.text}")
            sys.exit(1)
        
        # 3. Login
        log(f"[{role}] Logging in...")
        login_data = {"username": email, "password": "password123"}
        resp = client.post(f"{BASE_URL}/auth/login", data=login_data)
        if resp.status_code != 200:
            log(f"Login failed: {resp.text}")
            sys.exit(1)
            
        return resp.json()["access_token"]
        
    log(f"Unexpected registration status: {data}")
    sys.exit(1)

def main():
    log("Starting E2E Remote API Verification with Verbose Logging...")
    
    with httpx.Client(timeout=TIMEOUT) as client:
        # --- Seller Setup ---
        seller_email = generate_email("seller", f"sell-{uuid.uuid4().hex[:4]}.com")
        seller_token = register_user(client, seller_email, "SUPPLIER", f"SellerOrg_{uuid.uuid4().hex[:4]}", "FUEL_SUPPLIER")
        seller_headers = get_auth_headers(seller_token)
        log("Seller logged in successfully.")
        
        # --- Buyer Setup ---
        buyer_email = generate_email("buyer", f"buy-{uuid.uuid4().hex[:4]}.com")
        buyer_token = register_user(client, buyer_email, "BUYER", f"BuyerOrg_{uuid.uuid4().hex[:4]}", "SHIPPING_LINE")
        buyer_headers = get_auth_headers(buyer_token)
        log("Buyer logged in successfully.")
        
        # --- 1. Seller Creates Listing ---
        log("\n[Seller] Creating Listing...")
        listing_payload = {
            "region": "Rotterdam",
            "fuel_type": "Green Methanol",
            "fuel_grade": "Green",
            "quantity_mt": 1000,
            "price_per_mt_usd": 100.0, # Very low price to be unique/cheapest
            "availability_window": "Spot",
            "certifications": ["ISCC"]
        }
        log(f"   Payload: {listing_payload}")
        resp = client.post(f"{BASE_URL}/listings", json=listing_payload, headers=seller_headers)
        log_resp(resp)
        if resp.status_code != 201:
            sys.exit(1)
        
        listing = resp.json()
        listing_id = listing["id"]
        log(f"Listing created: {listing_id} @ $100.0")
        
        # --- 2. Buyer Finds Listing ---
        log("\n[Buyer] Searching for listing...")
        params = {"region": "Rotterdam", "fuel_type": "Green Methanol"}
        resp = client.get(f"{BASE_URL}/listings", params=params, headers=buyer_headers)
        # Don't log full list if huge, but maybe count
        log(f"   Found {len(resp.json())} listings matching criteria")
        listings = resp.json()
        
        target_listing = next((l for l in listings if l["id"] == listing_id), None)
        if not target_listing:
            log("Buyer could not find the created listing!")
            sys.exit(1)
        log("Buyer found the listing.")
        
        # --- 3. Buyer Creates Order ---
        log("\n[Buyer] Sending Order Request...")
        request_payload = {
            "listing_id": listing_id,
            "quantity_mt": 1000,
            "delivery_date": "2026-06-01T00:00:00",
            "accepted_terms": True
        }
        log(f"   Payload: {request_payload}")
        resp = client.post(f"{BASE_URL}/orders", json=request_payload, headers=buyer_headers)
        log_resp(resp)
        if resp.status_code != 200:
            sys.exit(1)
        
        order_data = resp.json()
        order_id = order_data["id"]
        log(f"Order created. ID: {order_id}")
        
        # --- 4. Verify Seller Notification ---
        log("\n[Seller] Checking Notifications...")
        time.sleep(2) # Allow async notification/DB propagation
        resp = client.get(f"{BASE_URL}/notifications/", headers=seller_headers)
        notifications = resp.json()
        log(f"   Found {len(notifications)} notifications")
        
        order_notif = next((n for n in notifications if n["type"] == "ORDER_UPDATE" and (n["data"].get("related_object_id") == order_id or n["data"].get("order_id") == order_id)), None)
        
        if not order_notif:
            log(f"Seller did NOT receive Order notification for {order_id}!")
            log("Latest notifications:")
            for n in notifications[:5]:
                 print(f" - ID: {n['id']}, Type: {n['type']}, Title: {n['title']}, Data: {n.get('data')}")
            sys.exit(1)
        
        log(f"Seller received notification: '{order_notif['title']}' - '{order_notif['message']}'")
        
        # --- 5. Seller Accepts Order ---
        log("\n[Seller] Accepting Order...")
        respond_payload = {"status": "ACCEPTED"}
        resp = client.put(f"{BASE_URL}/orders/{order_id}/respond", json=respond_payload, headers=seller_headers)
        log_resp(resp)
        if resp.status_code != 200:
            sys.exit(1)
        
        log("Seller accepted Order.")
        
        # --- 6. Verify Buyer Notification ---
        log("\n[Buyer] Checking Notifications...")
        time.sleep(2) 
        resp = client.get(f"{BASE_URL}/notifications/", headers=buyer_headers)
        notifications = resp.json()
        log(f"   Found {len(notifications)} notifications")
            
        accept_notif = next((n for n in notifications if n["type"] == "ORDER_UPDATE" and (n["data"].get("related_object_id") == order_id or n["data"].get("order_id") == order_id) and "accepted" in n["title"].lower()), None)
        
        if not accept_notif:
            log("Buyer did NOT receive Acceptance notification!")
            log("Latest notifications:")
            for n in notifications[:5]:
                 print(f" - ID: {n['id']}, Type: {n['type']}, Title: {n['title']}, Data: {n.get('data')}")
            sys.exit(1)
            
        log(f"Buyer received notification: '{accept_notif['title']}' - '{accept_notif['message']}'")
        
        log("\n✅ E2E Flow Verified Successfully (REMOTE)!")

if __name__ == "__main__":
    main()
