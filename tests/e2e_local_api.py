import httpx
import time
import uuid
import sys

# Configuration
BASE_URL = "http://localhost:8000/api"
TIMEOUT = 30

def generate_email(prefix, domain="test.com"):
    return f"{prefix}_{uuid.uuid4().hex[:8]}@{domain}"

def get_auth_headers(token):
    return {"Authorization": f"Bearer {token}"}

def register_user(client, email, role, org_name, org_type):
    print(f"[{role}] Registering {email}...")
    
    # 1. Register User
    reg_payload = {
        "email": email,
        "password": "password123",
        "first_name": "Test",
        "last_name": "User",
        "role": role
    }
    resp = client.post(f"{BASE_URL}/auth/register", json=reg_payload)
    if resp.status_code != 200:
        print(f"Registration failed: {resp.text}")
        sys.exit(1)
        
    data = resp.json()
    
    if data["status"] == "requires_org":
        print(f"[{role}] Creating Organization {org_name}...")
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
            print(f"Org creation failed: {resp.text}")
            sys.exit(1)
        
        # 3. Login
        print(f"[{role}] Logging in...")
        login_data = {"username": email, "password": "password123"}
        resp = client.post(f"{BASE_URL}/auth/login", data=login_data)
        if resp.status_code != 200:
            print(f"Login failed: {resp.text}")
            sys.exit(1)
            
        return resp.json()["access_token"]
        
    print(f"Unexpected registration status: {data}")
    sys.exit(1)

def main():
    print("Starting E2E Local API Verification...")
    
    with httpx.Client(timeout=TIMEOUT) as client:
        # --- Seller Setup ---
        seller_email = generate_email("seller", f"sell-{uuid.uuid4().hex[:4]}.com")
        seller_token = register_user(client, seller_email, "SUPPLIER", f"SellerOrg_{uuid.uuid4().hex[:4]}", "FUEL_SUPPLIER")
        seller_headers = get_auth_headers(seller_token)
        print("Seller logged in successfully.")
        
        # --- Buyer Setup ---
        buyer_email = generate_email("buyer", f"buy-{uuid.uuid4().hex[:4]}.com")
        buyer_token = register_user(client, buyer_email, "BUYER", f"BuyerOrg_{uuid.uuid4().hex[:4]}", "SHIPPING_LINE")
        buyer_headers = get_auth_headers(buyer_token)
        print("Buyer logged in successfully.")
        
        # --- 1. Seller Creates Listing ---
        print("\n[Seller] Creating Listing...")
        listing_payload = {
            "region": "Rotterdam",
            "fuel_type": "Green Methanol",
            "fuel_grade": "Green",
            "quantity_mt": 1000,
            "price_per_mt_usd": 100.0, # Very low price to be unique/cheapest
            "availability_window": "Spot",
            "certifications": ["ISCC"]
        }
        resp = client.post(f"{BASE_URL}/listings", json=listing_payload, headers=seller_headers)
        if resp.status_code != 201:
            print(f"Create listing failed: {resp.text}")
            sys.exit(1)
        
        listing = resp.json()
        listing_id = listing["id"]
        print(f"Listing created: {listing_id} @ $100.0")
        
        # --- 2. Buyer Finds Listing ---
        print("\n[Buyer] Searching for listing...")
        # Add a small delay for DB consistency if needed (usually instant)
        params = {"region": "Rotterdam", "fuel_type": "Green Methanol"}
        resp = client.get(f"{BASE_URL}/listings", params=params, headers=buyer_headers)
        listings = resp.json()
        
        target_listing = next((l for l in listings if l["id"] == listing_id), None)
        if not target_listing:
            print("Buyer could not find the created listing!")
            sys.exit(1)
        print("Buyer found the listing.")
        
        # --- 3. Buyer Creates Order ---
        print("\n[Buyer] Sending Order Request...")
        request_payload = {
            "listing_id": listing_id,
            "quantity_mt": 1000,
            "delivery_date": "2026-06-01T00:00:00",
            "accepted_terms": True
        }
        resp = client.post(f"{BASE_URL}/orders", json=request_payload, headers=buyer_headers)
        if resp.status_code != 200:
            print(f"Order Creation failed: {resp.text}")
            sys.exit(1)
        
        order_data = resp.json()
        order_id = order_data["id"]
        print(f"Order created. ID: {order_id}")
        
        # --- 4. Verify Seller Notification ---
        print("\n[Seller] Checking Notifications...")
        time.sleep(2) # Allow async notification/DB propagation
        resp = client.get(f"{BASE_URL}/notifications/", headers=seller_headers)
        if resp.status_code != 200:
            print(f"Get notifications failed: {resp.text}")
            sys.exit(1)
        
        notifications = resp.json()
        # Note: Notification data includes related_object_id
        order_notif = next((n for n in notifications if n["type"] == "ORDER_UPDATE" and (n["data"].get("related_object_id") == order_id or n["data"].get("order_id") == order_id)), None)
        
        if not order_notif:
            print(f"Seller did NOT receive Order notification for {order_id}!")
            print("Latest notifications:")
            for n in notifications:
                print(f" - ID: {n['id']}, Type: {n['type']}, Title: {n['title']}, Data: {n.get('data')}")
            sys.exit(1)
        
        print(f"Seller received notification: '{order_notif['title']}' - '{order_notif['message']}'")
        
        # --- 5. Seller Accepts Request ---
        print("\n[Seller] Accepting Order...")
        respond_payload = {"status": "ACCEPTED"}
        resp = client.put(f"{BASE_URL}/orders/{order_id}/respond", json=respond_payload, headers=seller_headers)
        if resp.status_code != 200:
            print(f"Respond to Order failed: {resp.text}")
            sys.exit(1)
        
        print("Seller accepted Order.")
        
        # --- 6. Verify Buyer Notification ---
        print("\n[Buyer] Checking Notifications...")
        time.sleep(2) 
        resp = client.get(f"{BASE_URL}/notifications/", headers=buyer_headers)
        if resp.status_code != 200:
            print(f"Get notifications failed: {resp.text}")
            sys.exit(1)
            
        notifications = resp.json()
        accept_notif = next((n for n in notifications if n["type"] == "ORDER_UPDATE" and (n["data"].get("related_object_id") == order_id or n["data"].get("order_id") == order_id) and "accepted" in n["title"].lower()), None)
        
        if not accept_notif:
            print("Buyer did NOT receive Acceptance notification!")
            print("Latest notifications:", [n["title"] for n in notifications[:3]])
            sys.exit(1)
            
        print(f"Buyer received notification: '{accept_notif['title']}' - '{accept_notif['message']}'")
        
        print("\n✅ E2E Flow Verified Successfully (LOCAL)!")

if __name__ == "__main__":
    main()
