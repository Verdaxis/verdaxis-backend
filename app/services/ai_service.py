import google.generativeai as genai
from app.config import settings

# Initialize Gemini
if settings.GEMINI_API_KEY:
    genai.configure(api_key=settings.GEMINI_API_KEY)

async def chat_with_copilot(message: str, history: list | None = None):
    history = history or []
    if not settings.GEMINI_API_KEY:
        return "AI Service is not configured (Missing API Key)."
        
    model = genai.GenerativeModel('gemini-pro')
    
    # Simple interaction for now
    response = model.generate_content(message)
    return response.text

async def analyze_document(file_content):
    # Placeholder for Vision API
    return {"fuel_type": "Methanol", "quantity": 500}
