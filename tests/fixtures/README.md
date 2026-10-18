These one-second 440 Hz tones and the small red embedded cover are synthetic test
data generated with FFmpeg. They contain no Spotify or other third-party recordings.
Tags: title "Local fixture", artist "Lumen Tests", album "Owned audio".

Recreate a tone with:

    ffmpeg -f lavfi -i sine=frequency=440:duration=1 -metadata title="Local fixture" -metadata artist="Lumen Tests" -metadata album="Owned audio" tone.wav

Use .flac, .mp3, .ogg, .opus or .m4a to generate the other containers/codecs.
