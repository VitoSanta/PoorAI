# ADR-005: Hardware-aware planning

**Status:** Accepted. **Decision:** planning uses `HardwareProfile`, fresh `RuntimeSnapshot`, model/deployment metadata, and compatible calibration.

Fixed memory-to-context rules are rejected because actual backend state and quantization matter. Consequence: planning can decline or downgrade execution when evidence is missing or resources are unsafe.
