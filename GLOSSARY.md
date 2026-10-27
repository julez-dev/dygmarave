# Dygmarave

A daemon that renders the audio playing on this computer as live colour on a Dygma Raise keyboard. This context is the vocabulary of that render: audio becomes energy, energy becomes colour, colour lands on keys.

## Language

### Signal

**Channel**:
One of the two sides of a stereo signal, left or right. Each half of the keyboard renders exactly one channel.
_Avoid_: Side, stream, output

**Band**:
A contiguous range of audible frequencies treated as a single unit of energy.
_Avoid_: Bin, bucket, slice

**Energy**:
The current strength of a band, from silence to full.
_Avoid_: Amplitude, magnitude

**Level**:
The overall loudness of a channel, shown on that channel's thumb cluster.
_Avoid_: Volume, gain, loudness

**Beat**:
A sharp rise in the bass that reads as rhythmic onset.
_Avoid_: Hit, pulse, kick (these name the source, not the event)

### Layout

**Half**:
One physical side of the split keyboard.
_Avoid_: Side, board, pane

**Matrix**:
The alphanumeric key area of a half, excluding the thumb clusters. The matrix is where the spectrum is drawn.
_Avoid_: Grid, main area, block

**Band sequence**:
The order in which bands are assigned to matrix keys: down one column, back up the next, so neighbouring bands occupy neighbouring keys. Each half's matrix holds one distinct band per key.
_Avoid_: Reading order (rejected: it puts a frequency discontinuity at every row boundary)

**Thumb cluster**:
The thumb-reachable keys at the inner edge of a half. Reserved for level and beat, never for bands.
_Avoid_: Thumbs, cluster keys

**Underglow**:
The light pipes running around and beneath the key well. They are LEDs like any other but carry no band, so they stay dark.
_Avoid_: Edge lights, glow strip

### Render

**Baseline theme**:
The LED state the keyboard showed before the daemon took over. Restored on a clean exit.
_Avoid_: Original state, saved theme

**Frame**:
One complete colouring of the whole keyboard, produced from one audio window.
_Avoid_: Tick, update, refresh

**Ramp**:
The mapping from energy to colour, running from black through red and orange toward white.
_Avoid_: Palette, gradient, colormap, heatmap

**Attack**:
How quickly a band's displayed energy rises. Attack is immediate.
_Avoid_: Smoothing, rise time

**Decay**:
How quickly a band's displayed energy falls once the band goes quiet. Decay is slower than attack.
_Avoid_: Falloff, release, smoothing

**AGC**:
Automatic gain control: each band is scaled against its own recent maximum, so quietly mastered music still fills the board.
_Avoid_: Normalization, auto-gain, limiting

**Gate**:
The silence threshold below which the board goes black rather than showing amplified noise.
_Avoid_: Cutoff, floor, mute
