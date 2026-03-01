from fastapi import APIRouter, Depends, HTTPException, UploadFile, File
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.compliance import ComplianceLedger
from app.schemas.compliance import ComplianceLedgerResponse
from app.models.user import User
from app.routers.auth_simple import get_current_user
from typing import List, Annotated
from datetime import datetime
from app.services.ai_service import analyze_document # To be implemented

router = APIRouter()

@router.get("/compliance/ledger", response_model=List[ComplianceLedgerResponse])
async def get_ledger(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    stmt = select(ComplianceLedger).where(ComplianceLedger.organization_id == current_user.organization_id)
    result = await db.execute(stmt)
    entries = result.scalars().all()
    return entries

@router.post("/compliance/verify")
async def verify_document(
    current_user: Annotated[User, Depends(get_current_user)],
    file: UploadFile = File(...)
):
    # 1. Upload to Cloud Storage (Mocked for now)
    # file_url = await upload_to_s3(file)
    
    # 2. Call AI Service to extract data
    # extraction = await analyze_document(file.file)
    
    return {
        "status": "processing", 
        "message": "Document uploaded for AI verification",
        "filename": file.filename
    }
