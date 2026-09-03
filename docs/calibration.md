# Calibration

Calibration measures stable operating points for a particular deployment and machine, producing a signed/hashed `CalibrationProfile`.

## Procedure

Warm the model; snapshot host/backend; run fixed prompts at a context ladder; measure first-token latency, generation rate, peak memory/backend reported allocation, errors, and recovery. Repeat at least three times, randomize order after warm-up, and record raw samples. A stable point meets configured success rate, latency, and no-pressure thresholds. The profile stores maxima with confidence/variance—not invented capacity.

## Invalidations

Invalidate on model digest, quantization, provider/backend version, relevant model parameters, hardware compatibility key, or calibration harness change. Fresh backend state can temporarily downgrade a profile without invalidating it.

Calibration has no task-success claim; task evaluation is separate.

## Implementation notes

Warm-up is per tier, not per run: a backend reloads the model when the context size changes, so one warm-up leaves every later tier's first sample carrying a reload. A sample the backend reports as having loaded the model disqualifies its tier; where no load duration is reported this stays unknown rather than assumed warm.

Generation rate uses backend-reported token counts and durations where available, falling back to a local chunk rate otherwise, and every sample records which source it used.

Profiles store the thresholds they were judged against, and validation refuses to hold a point that fails them. A refusal is persisted like a profile, carrying its samples and the criteria each tier failed.

A ladder of `num_ctx` values with a fixed short prompt measures allocation, not occupancy: it establishes that a tier can be served, not what a full context costs.

**Closed as of 2026-09-03.** The prompt now fills three quarters of the tier — not all of it, since the reply needs somewhere to go and a full context measures a refusal rather than a cost — with varied filler, because a run of identical tokens compresses in ways a real prompt does not. Pressure is sampled after the reply as well as before, since reading it before generating is the one moment the cost is guaranteed not to have been paid. A needle placed at the start is asked for at the end: a tier where it comes back held the context, and one where it does not was allocated and then not used. The occupancy the backend reports is recorded against the tier requested.

Every profile measured under the older ladder is invalidated by the harness revision, which is that gate working: they measured something else.
