"""Saved "Quick sync" presets (folder + destination + upload options)."""

import uuid
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from ..database import SyncPreset, get_db

router = APIRouter()


class PresetRequest(BaseModel):
    name: str = Field(..., min_length=1, max_length=120)
    source_path: str = Field(..., min_length=1)
    destination: str = Field(..., min_length=1)
    options: Optional[Dict[str, Any]] = None


@router.get("/")
async def list_presets(db: Session = Depends(get_db)) -> List[dict]:
    rows = db.query(SyncPreset).order_by(SyncPreset.name.asc()).all()
    return [row.to_dict() for row in rows]


@router.post("/")
async def save_preset(request: PresetRequest, db: Session = Depends(get_db)) -> dict:
    """Create a preset, or overwrite the one with the same name."""
    name = request.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail="Preset name is required")
    now = datetime.now(timezone.utc)
    preset = db.query(SyncPreset).filter(SyncPreset.name == name).first()
    if preset is None:
        preset = SyncPreset(preset_id=uuid.uuid4().hex, name=name, created_at=now)
        db.add(preset)
    preset.source_path = request.source_path.strip()
    preset.destination = request.destination.strip()
    preset.options = request.options or {}
    preset.updated_at = now
    db.commit()
    db.refresh(preset)
    return preset.to_dict()


@router.delete("/{preset_id}")
async def delete_preset(preset_id: str, db: Session = Depends(get_db)) -> dict:
    preset = db.query(SyncPreset).filter(SyncPreset.preset_id == preset_id).first()
    if preset is None:
        raise HTTPException(status_code=404, detail="Preset not found")
    db.delete(preset)
    db.commit()
    return {"message": "Preset deleted", "preset_id": preset_id}
