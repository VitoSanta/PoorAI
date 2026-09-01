# Hardware Profiling

## Requirement

Capture a versioned `HardwareProfile` before calibration and execution. Include OS/build, architecture, CPU topology, total and currently available memory, accelerator/device capability, storage free space, and power/thermal state where reliably available. Never log serial numbers or user names.

## Collection

Implement a `HardwareProbe` trait. macOS adapter uses documented system interfaces/commands, parsing into typed units; Linux and Windows are later adapters. Record raw probe provenance, timestamp, probe version, unavailable fields, and units. A failed optional probe is `unknown`, not zero.

## Policy

**Heuristic:** reserve a configurable memory floor for OS, editor, and concurrent services; calculate using measured pressure and calibration, not a hard-coded percentage. Refuse calibration/execution when free storage cannot hold artifacts or memory pressure exceeds policy. Hardware changes invalidate calibration by compatibility key.
