# Calibration

Calibration measures stable operating points for a particular deployment and machine, producing a signed/hashed `CalibrationProfile`.

## Procedure

Warm the model; snapshot host/backend; run fixed prompts at a context ladder; measure first-token latency, generation rate, peak memory/backend reported allocation, errors, and recovery. Repeat at least three times, randomize order after warm-up, and record raw samples. A stable point meets configured success rate, latency, and no-pressure thresholds. The profile stores maxima with confidence/variance—not invented capacity.

## Invalidations

Invalidate on model digest, quantization, provider/backend version, relevant model parameters, hardware compatibility key, or calibration harness change. Fresh backend state can temporarily downgrade a profile without invalidating it.

Calibration has no task-success claim; task evaluation is separate.
