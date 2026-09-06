# Synthetic media fixture

`synthetic.mp4` is a four-second, silent 320×240 H.264 color test pattern,
generated locally for `create_trial.py`. It contains no downloaded media.
The generator copies this tiny seed; users do not need FFmpeg installed.

Reproduction (FFmpeg is needed only to regenerate the seed):

```sh
ffmpeg -nostdin -v error -f lavfi -i testsrc2=size=320x240:rate=4:duration=4 -c:v libx264 -threads 1 -g 4 -movflags +faststart synthetic.mp4
```
