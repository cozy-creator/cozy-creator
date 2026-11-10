use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixtures {
    dir: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let ffmpeg = std::env::var_os("COZY_MEDIA_FFMPEG").expect("set COZY_MEDIA_FFMPEG").into();
        let ffprobe = std::env::var_os("COZY_MEDIA_FFPROBE").expect("set COZY_MEDIA_FFPROBE").into();
        let root = std::env::var_os("COZY_MEDIA_TEST_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = root.join(format!("cozy-media-{}-{stamp}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        Self { dir, ffmpeg, ffprobe }
    }

    fn make(&self, name: &str, graph: &str, args: &[&str]) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let done = Command::new(&self.ffmpeg)
            .args(["-v", "error", "-y", "-threads", "1", "-filter_threads", "1", "-f", "lavfi", "-i", graph])
            .args(args).args(["-threads", "1"]).arg(&path).output().unwrap();
        assert!(done.status.success(), "fixture {name}: {}", String::from_utf8_lossy(&done.stderr));
        path
    }

    fn probe(&self, path: &Path) -> Output {
        Media::new(&self.ffprobe).args(["-v", "error", "-threads", "1"]).input(path)
            .args(["-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"]).output().unwrap()
    }

    fn decode(&self, path: &Path) -> Output {
        Media::new(&self.ffmpeg).args(["-v", "error", "-threads", "1"]).input(path)
            .args(["-filter_threads", "1", "-threads", "1", "-f", "null", "-"]).output().unwrap()
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn succeeds(done: &Output) {
    assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
}

#[test]
#[ignore = "requires COZY_MEDIA_FFMPEG and COZY_MEDIA_FFPROBE"]
fn file_inputs_refuse_downloaded_playlists_and_keep_real_media() {
    let f = Fixtures::new();
    let png = f.make("control.png", "color=c=red:s=32x24:r=2", &["-frames:v", "1", "-update", "1"]);
    let video = f.make("marker.mp4", "testsrc2=s=64x48:r=2:d=1", &["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
    f.make("marker.ts", "testsrc2=s=64x48:r=2:d=1", &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mpegts"]);
    let audio = f.make("control.wav", "sine=frequency=440:sample_rate=8000:duration=0.25", &["-c:a", "pcm_s16le"]);
    for (path, shape) in [(&png, "32,24"), (&video, "64,48")] {
        let done = f.probe(path);
        succeeds(&done);
        assert_eq!(String::from_utf8_lossy(&done.stdout).trim(), shape);
        succeeds(&f.decode(path));
    }
    let sound = Media::new(&f.ffprobe).args(["-v", "error", "-threads", "1"]).input(&audio)
        .args(["-show_entries", "stream=sample_rate", "-of", "csv=p=0"]).output().unwrap();
    succeeds(&sound);
    assert_eq!(String::from_utf8_lossy(&sound.stdout).trim(), "8000");
    succeeds(&f.decode(&audio));

    let hls = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nmarker.ts\n#EXT-X-ENDLIST\n";
    for (name, bytes, format) in [
        ("download-hls.mp4.part", hls, "hls"),
        ("download.m3u8", hls, "hls"),
        ("download-concat.mp4.part", "ffconcat version 1.0\nfile 'marker.mp4'\n", "concat"),
    ] {
        let path = f.dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        // The references are real and readable. The helper, rather than import's sniff, must refuse them.
        let bare = Command::new(&f.ffprobe).args(["-v", "error", "-threads", "1", "-f", format, "-i"]).arg(&path)
            .args(["-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"]).output().unwrap();
        succeeds(&bare);
        assert!(String::from_utf8_lossy(&bare.stdout).contains("64,48"));
        assert!(!f.probe(&path).status.success(), "ffprobe followed {name}");
        assert!(!f.decode(&path).status.success(), "ffmpeg followed {name}");
    }
}

#[test]
#[ignore = "requires COZY_MEDIA_FFMPEG and COZY_MEDIA_FFPROBE"]
fn a_literal_percent_parent_never_selects_a_neighboring_image() {
    let f = Fixtures::new();
    let literal = f.make("literal%02d/same.png", "color=c=red:s=32x24:r=2", &["-frames:v", "1", "-update", "1"]);
    f.make("literal00/same.png", "color=c=blue:s=80x56:r=2", &["-frames:v", "1", "-update", "1"]);
    let probed = f.probe(&literal);
    if probed.status.success() {
        assert_eq!(String::from_utf8_lossy(&probed.stdout).trim(), "32,24", "image2 expanded the literal path");
    }
    let decoded = Media::new(&f.ffmpeg).args(["-v", "error", "-threads", "1"]).input(&literal)
        .args(["-filter_threads", "1", "-threads", "1", "-frames:v", "1", "-f", "framehash", "-"]).output().unwrap();
    if decoded.status.success() {
        assert!(String::from_utf8_lossy(&decoded.stdout).contains("#dimensions 0: 32x24"), "ffmpeg decoded the neighbor: {}", String::from_utf8_lossy(&decoded.stdout));
    }
}

#[test]
#[ignore = "requires COZY_MEDIA_FFMPEG and COZY_MEDIA_FFPROBE"]
fn joined_reads_the_generated_transport_stream_list() {
    let f = Fixtures::new();
    let piece = f.make("piece.ts", "testsrc2=s=64x48:r=2:d=1", &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mpegts"]);
    let list = f.dir.join("pieces.txt");
    std::fs::write(&list, format!("file '{}'\n", piece.display().to_string().replace('\'', r"'\''"))).unwrap();
    let film = f.dir.join("film.mp4");
    let done = Media::new(&f.ffmpeg).args(["-v", "error", "-threads", "1"]).joined(&list)
        .args(["-c", "copy", "-f", "mp4"]).arg(&film).output().unwrap();
    succeeds(&done);
    let shape = f.probe(&film);
    succeeds(&shape);
    assert_eq!(String::from_utf8_lossy(&shape.stdout).trim(), "64,48");
}

#[test]
#[ignore = "requires COZY_MEDIA_FFMPEG and COZY_MEDIA_FFPROBE"]
fn stills_and_animations_keep_their_decoded_frames() {
    let f = Fixtures::new();
    for (name, args, frames) in [
        ("still.jpg", &["-frames:v", "1", "-update", "1"][..], 1),
        ("still.webp", &["-frames:v", "1", "-c:v", "libwebp"][..], 1),
        ("animation.gif", &["-frames:v", "2", "-loop", "0"][..], 2),
        ("animation.apng", &["-frames:v", "2", "-plays", "0"][..], 2),
        ("still.tiff", &["-frames:v", "1", "-update", "1"][..], 1),
        ("still.bmp", &["-frames:v", "1", "-update", "1"][..], 1),
        ("alpha.png", &["-frames:v", "1", "-update", "1"][..], 1),
    ] {
        let graph = if name == "alpha.png" { "color=c=red@0.5:s=32x24:r=2,format=rgba" } else { "testsrc2=s=32x24:r=2:d=1" };
        let path = f.make(name, graph, args);
        let probed = Media::new(&f.ffprobe).args(["-v", "error", "-threads", "1"]).input(&path)
            .args(["-count_frames", "-show_entries", "stream=width,height,pix_fmt,nb_read_frames", "-of", "csv=p=0"]).output().unwrap();
        succeeds(&probed);
        let text = String::from_utf8_lossy(&probed.stdout);
        let parts: Vec<_> = text.trim().split(',').collect();
        assert_eq!(&parts[..2], ["32", "24"], "{name}");
        assert_eq!(parts.last().unwrap().parse::<usize>().unwrap(), frames, "{name}");
        if name == "alpha.png" {
            assert_eq!(parts[2], "rgba");
        }
        let decoded = Media::new(&f.ffmpeg).args(["-v", "error", "-threads", "1"]).input(&path)
            .args(["-filter_threads", "1", "-threads", "1", "-f", "framehash", "-"]).output().unwrap();
        succeeds(&decoded);
        let text = String::from_utf8_lossy(&decoded.stdout);
        assert_eq!(text.lines().filter(|line| !line.starts_with('#') && !line.is_empty()).count(), frames, "{name}");
    }
}
