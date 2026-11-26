# Stereo continuity across a CUE chapter

The offline chapter test now waits until the decoder scheduler has queued the full 9,600-frame stereo fixture. It compares every output frame with the source, including the transition at physical frame 5,120. A separate silent-output case verifies the pre-fader tap and output silence.

## Reproducing the failure

On Windows, twelve runs of the instrumented test suite used six concurrent processes, each with 32 test threads, sharing two logical processors. The original mock backend rendered 128 frames at a time and slept one millisecond between callbacks. The decoder ran on another thread. The fixture is 48 kHz, so 128 frames represent about 2.67 milliseconds of audio.

Seven chapter tests failed. One reached the test deadline while the voice was still playing. In six completed captures, the output ran out of decoded samples before the CUE boundary. Comparing both channel values against the fixture identified the original physical frame on either side of each gap. All six preserved the sequence 5,118 through 5,122. In one capture, 5,248 inserted silent frames moved source frame 5,120 to output offset 10,368.

In a separate control, the decoder paused at a packet boundary after queuing 1,152 frames. Advancing the backend to 1,408 before releasing the decoder inserted exactly 256 silent frames. All 9,600 source frames remained in order, but the original comparison failed at offset 4,608. Releasing the same decoder and waiting for the queue to fill before the first callback passed.

## Scope of the fix

The fixture fits in Kira 0.12.6's 16,384-frame ring. The test observes the scheduler dropping its owned decoder after the final enqueue and checks the decoded extent and error state. The packet decode counter alone can advance before the scheduler finishes enqueueing a packet. The timeout bounds a failed readiness check. Once ready, the test advances a bounded number of blocks and compares the whole disc exactly. The twelve suites under the same constrained load all passed after this change.

Two of the six failing captures also had a genuine one-frame discontinuity during recovery from starvation, at 3,513 to 3,515 and 4,122 to 4,124. Kira's streaming renderer checks available frames at callback entry and can drain its interpolation history when the ring empties within the callback. This finding concerns recovery from an underrun. No discontinuity was observed at the chapter boundary itself. Real-device underrun recovery remains unqualified. The test synchronization change has no effect on the production audio path.
