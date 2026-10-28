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
A named ordering that assigns each band to one matrix key, so neighbouring bands occupy neighbouring keys. The `snake` sequence is the v0.1 default: it runs along rows, alternating direction and starting at the inner edge. A layout is one named band sequence.
_Avoid_: Reading order (rejected: it puts a frequency discontinuity at every row boundary)

**Thumb cluster**:
The thumb-reachable keys at the inner edge of a half. Reserved for level and beat, never for bands.
_Avoid_: Thumbs, cluster keys

**Underglow**:
The light pipes running around and beneath the key well. They are LEDs like any other but carry no band, so they stay dark.
_Avoid_: Edge lights, glow strip

### Look

**Layout**:
One of the named band sequences that decides which matrix key carries each band.
_Avoid_: Arrangement, mapping

**Palette**:
One of the named ramps that decides which colour an energy level becomes.
_Avoid_: Gradient, colormap

**Motion**:
An additive modulation over the composed matrix, such as a sweep or sparkle. Motion never chooses colours and never touches the thumb clusters.
_Avoid_: Effect, animation

**Switch**:
A logged change of one look axis made by a random timer. A fixed selection resolved once at startup is not a switch.
_Avoid_: Transition, update

### Render

**Baseline theme**:
The LED state the keyboard showed before the daemon took over. Restored on a clean exit.
_Avoid_: Original state, saved theme

**Frame**:
One complete colouring of the whole keyboard, produced from one audio window.
_Avoid_: Tick, update, refresh

**Ramp**:
A mapping from energy to colour, running from black upward. A palette is a named ramp; the default `ember` runs from black through red and orange toward white.
_Avoid_: Gradient, colormap, heatmap

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
