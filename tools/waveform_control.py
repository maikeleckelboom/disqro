"""Write the 24-second stereo PCM fixture used by the waveform research."""

import argparse
import math
from pathlib import Path
import struct
import wave


RATE = 44100


def tone(hz, seconds):
    return math.sin(math.tau * hz * seconds)


def channels(frame):
    second, within = divmod(frame, RATE)
    t = within / RATE
    if second in (1, 2, 3):
        sample = 0.4 * tone((70, 880, 6300)[second - 1], t)
        return sample, sample
    if second == 4:
        sample = 0.22 * sum(tone(hz, t) for hz in (70, 880, 6300))
        return sample, sample
    if second == 5:
        # The phase integral of a linear 40–10,000 Hz sweep.
        sample = 0.5 * tone(40 + 4980 * t, t)
        return sample, sample
    if second == 7 and within == 37:
        return 0.90, -0.75
    if second == 9:
        body = 0.72 * math.exp(-7 * t) * tone(70, t)
        attack = 0.24 * math.exp(-450 * t) * tone(6300, t)
        return body + attack, 0.8 * body - attack
    if second == 11:
        sample = 1.6 * tone(160, t)
        return sample, 0.75 * sample
    if second == 13:
        sample = 0.005 * tone(880, t)
        return sample, sample
    if second == 15:
        sample = 0.52 * tone(70, t)
        return sample, -sample
    if second == 17:
        return 0.4 * tone(70, t), 0.4 * tone(6300, t)
    if 19 <= second < 23:
        beat = (second + t) % 0.5
        bass = 0.58 * math.exp(-16 * beat) * tone(68, beat)
        treble = 0.22 * math.exp(-100 * (t % 0.125)) * tone(5900, t)
        middle = 0.15 * tone(390, t) * tone(4, t)
        return bass + treble + middle, bass + 0.8 * treble - middle
    return 0.0, 0.0


def pcm(sample):
    return max(-32768, min(32767, round(sample * 32768)))


def write_fixture(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as output, wave.open(output, "wb") as recording:
        recording.setparams((2, 2, RATE, 24 * RATE, "NONE", "not compressed"))
        for second in range(24):
            frames = bytearray(RATE * 4)
            for within in range(RATE):
                left, right = channels(second * RATE + within)
                struct.pack_into("<hh", frames, within * 4, pcm(left), pcm(right))
            recording.writeframesraw(frames)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    write_fixture(parser.parse_args().output)
