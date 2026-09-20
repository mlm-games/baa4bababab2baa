# Local golden fixtures (gitignored). To Regenerate, erun:
#
# ffmpeg -v error -i ~/Videos/Big_Buck_Bunny_360_10s_1MB.webm -frames:v 1 -f rawvideo -pix_fmt nv12 assets-test/bbb360-nv12.bin -y
# ffmpeg -v error -i ~/Videos/sample-3s.wav -t 1 -f s16le -ac 2 -ar 48000 assets-test/sample-1s-s16-44100-stereo.bin -y
# ffmpeg -v error -i ~/Videos/sample-3s.wav -t 1 -f f32le -ac 2 -ar 48000 assets-test/sample-1s-f32-44100-stereo.bin -y
#

assets-test/bbb360-nv12.bin: 345600 bytes md5=8061a03d7141827010d3a4ae1c8c1745 [OK] (Big_Buck_Bunny_360_10s_1MB.webm frame 0 via ffmpeg -pix_fmt nv12)
assets-test/sample-1s-s16-44100-stereo.bin: 176400 bytes md5=3308b301b2d38d25bb57bc680dcd3070 [OK] (sample-3s.wav first 1s via ffmpeg -f s16le -ac 2 -ar 48000)
assets-test/sample-1s-f32-44100-stereo.bin: 352800 bytes md5=6f709677c0580221da3a57758066c15b [OK] (sample-3s.wav first 1s via ffmpeg -f f32le -ac 2 -ar 48000)
