import requests
import json
import sys
import datetime
from jose import jwt

# Use Local URL
API_URL = "http://localhost:8000/api"
# Matches config.py default
JWT_SECRET = "dev-secret-key-not-for-production" 
JWT_ALGORITHM = "HS256"

def create_local_token(email, role, user_id):
    # We need a user_id. Since we can't query the remote DB, we have to guess or assume.
    # OR, we might hope the backend looks up by email if sub ID isn't found?
    # Checking auth.py: 
    #   stmt = select(User).where(User.email == email)
    #   user = result.scalar_one_or_none()
    # It extracts 'email' from payload.get("email").
    # It does NOT seem to rely on 'sub' strictly for ID lookup if it's checking email.
    # Lines 142: select(User).where(User.email == email)
    
    payload = {
        "email": email,
        "sub": user_id, # Can be dummy if lookup is by email?
        "role": role,
        "exp": datetime.datetime.utcnow() + datetime.timedelta(hours=1)
    }
    token = jwt.encode(payload, JWT_SECRET, algorithm=JWT_ALGORITHM)
    return token

def main():
    print(f">>> Starting Purchase Flow Test against {API_URL}")

    # 1. Mint Tokens
    # We use dummy UUIDs. Hopefully the backend uses email to find the user record correctly.
    buyer_token = create_local_token("buyer1@verdaxis.com", "BUYER", "00000000-0000-0000-0000-000000000001")
    supplier_token = create_local_token("supplier1@verdaxis.com", "SUPPLIER", "00000000-0000-0000-0000-000000000002")
    
    buyer_headers = {"Authorization": f"Bearer {buyer_token}"}
    supplier_headers = {"Authorization": f"Bearer {supplier_token}"}
    
    print("   Tokens Minted.")

    # 2. Check Auth / Ports
    print("\n2. Verifying Buyer Auth & Fetching Ports...")
    try:
        res = requests.get(f"{API_URL}/ports", headers=buyer_headers)
        if res.status_code != 200:
            print(f"   Auth/Ports Failed: {res.status_code} {res.text}")
            sys.exit(1)
        ports = res.json()
        print(f"   Success! Found {len(ports)} ports.")
        port_id = ports[0]["id"]
    except Exception as e:
        print(f"   Connection Failed: {e}")
        sys.exit(1)

    print("\n   Fetching Vessels...")
    res = requests.get(f"{API_URL}/vessels", headers=buyer_headers)
    vessels = res.json()
    if not vessels:
         print("   No vessels found.")
         sys.exit(1)
    vessel_id = vessels[0]["id"]
    print(f"   Found vessel: {vessels[0]['name']}")

    # 3. Create Quote Request
    print("\n3. Buyer Creates Quote Request...")
    quote_payload = {
        "port_id": port_id,
        "fuel_type": "Methanol",
        "quantity_mt": 500,
        "delivery_window_start": "2026-03-01", 
        "delivery_window_end": "2026-03-02",
        "vessel_id": vessel_id
    }
    
    res = requests.post(f"{API_URL}/quotes", json=quote_payload, headers=buyer_headers)
    if res.status_code != 200:
        print(f"   Create Quote Failed: {res.text}")
        sys.exit(1)
    quote_request = res.json()
    quote_id = quote_request["id"]
    print(f"   Quote Request Created: {quote_id}")

    # 4. Supplier Lists Quotes
    print("\n4. Supplier Lists Quotes...")
    # NOTE: Backend logic for suppliers:
    # "Suppliers see requests they have offered on OR all pending requests (Marketplace) - Simplified for demo: Show all"
    
    res = requests.get(f"{API_URL}/quotes", headers=supplier_headers)
    quotes = res.json()
    found = False
    for q in quotes:
        if q["id"] == quote_id:
            found = True
            break
    
    if not found:
        print("   Quote Request NOT found in supplier list!")
        sys.exit(1)
    print("   Quote Request found in Supplier list.")

    # 5. Supplier Creates Offer
    print("\n5. Supplier Creates Offer...")
    offer_payload = {
        "price_per_mt_usd": 650.00,
        "valid_until": "2026-03-10T12:00:00Z",
        "terms_and_conditions": "FOB Rotterdam"
    }
    url = f"{API_URL}/quotes/{quote_id}/offers"
    print(f"   POST {url}")
    res = requests.post(url, json=offer_payload, headers=supplier_headers)
    
    if res.status_code == 404:
        # Try with trailing slash
        url_slash = f"{url}/"
        print(f"   404 encountered. Retrying with {url_slash}")
        res = requests.post(url_slash, json=offer_payload, headers=supplier_headers)

    if res.status_code != 200:
        print(f"   Create Offer Failed: {res.status_code} {res.text}")
        print(f"   URL: {res.url}")
        sys.exit(1)
    offer = res.json()
    offer_id = offer["id"]
    print(f"   Offer Created: {offer_id}")

    # 6. Buyer Checks Offers
    print("\n6. Buyer Checks Offers...")
    res = requests.get(f"{API_URL}/quotes", headers=buyer_headers)
    quotes = res.json()
    my_quote = next((q for q in quotes if q["id"] == quote_id), None)
    
    if not my_quote:
        print("   Quote not found for buyer!")
        sys.exit(1)
        
    if not my_quote.get("offers"):
        print("   Offer list is empty!")
        sys.exit(1)
    
    print("   Offer received by Buyer.")

    # 7. Buyer Accepts Offer
    print("\n7. Buyer Accepts Offer...")
    res = requests.put(f"{API_URL}/quotes/{quote_id}/accept/{offer_id}", headers=buyer_headers)
    if res.status_code != 200:
        print(f"   Accept Offer Failed: {res.text}")
        sys.exit(1)
    
    confirmed_quote = res.json()
    if confirmed_quote["status"] == "Confirmed":
        print("   Quote Status: Confirmed!")
        print("   Flow Completed Successfully.")
    else:
        print(f"   Quote Status Mismatch: {confirmed_quote['status']}")
        sys.exit(1)

if __name__ == "__main__":
    main()
