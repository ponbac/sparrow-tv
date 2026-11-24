use std::time::Duration;

use image::{
    ExtendedColorType, Frame, ImageEncoder, Rgba, RgbaImage,
    codecs::{gif::GifEncoder, png::PngEncoder, webp::WebPEncoder},
};
use sparrow_channel_icons::{ChannelIconClient, ChannelIconError};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

// Tiny synthetic red images encoded by libwebp. The alpha fixture uses the
// compressed ALPH mode, with a uniform translucent 32x32 canvas.
const SIMPLE_WEBP: &[u8] = b"RIFF\x3c\0\0\0WEBPVP8 \x30\0\0\0\
    \xd0\x01\0\x9d\x01*\x02\0\x02\0\x02\0\x34\x25\xa0\x02\x74\xba\x01\xf8\
    \0\x03\xb0\0\xfe\xf0\xc4\x0b\xff\x20\xb9\x61\x75\xc8\xd7\xff\x20\x3f\
    \xe4\x07\xfc\x80\xff\xf8\xf2\0\0\0";
const COMPRESSED_ALPHA_WEBP: &[u8] = b"RIFFp\0\0\0WEBP\
    VP8X\x0a\0\0\0\x10\0\0\0\x1f\0\0\x1f\0\0\
    ALPH\x0a\0\0\0\x01\x07\x50\xc0\x88\x08\x44\x44\xff\x03\
    VP8 @\0\0\0\x50\x03\0\x9d\x01*\x20\0\x20\0\x3e\x91\x42\x9c\x4a\x25\
    \xa3\xa2\xa1\xa8\x08\0\xb0\x12\x09\x65\0\xc6\xaa\x80\0\x40\x51\x1c\0\0\
    \xfe\xee\xa6\x3f\xfe\xc5\x9c\xb6\x05\xe3\xff\xfb\x9c\x0f\xfb\x9c\x0f\
    \xfb\x9c\x0f\xe3\x6c\xa4\x27\x67\x0a\0\0";

#[tokio::test]
async fn follows_five_anonymous_redirects_and_downscales_the_raster() {
    let pixels = [10, 70, 200, 255].repeat(512 * 128);
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&pixels, 512, 128, ExtendedColorType::Rgba8)
        .unwrap();

    let target = Fixture::serve(vec![response(
        "200 OK",
        "Content-Type: text/plain\r\n",
        &png,
    )])
    .await;
    let target_authority = target.url.strip_prefix("http:").unwrap();
    let origin = Fixture::serve(vec![
        redirect("/one"),
        redirect("/two"),
        redirect("/three"),
        redirect("/four"),
        redirect(&format!("{target_authority}/image")),
    ])
    .await;

    let client = ChannelIconClient::new().unwrap();
    let icon = client
        .fetch(&format!("{}/start?private-fixture=1", origin.url))
        .await
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(icon.source_dimensions(), (512, 128));
    let (width, height, bgra) = icon.into_bgra();
    assert_eq!((width, height, bgra.len()), (256, 64, 256 * 64 * 4));
    assert_eq!(&bgra[..4], &[200, 70, 10, 255]);

    let requests = [origin.finish().await, target.finish().await].concat();
    for (request, path) in requests.iter().zip([
        "/start?private-fixture=1",
        "/one",
        "/two",
        "/three",
        "/four",
        "/image",
    ]) {
        assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
        let lowercase = request.to_ascii_lowercase();
        for incoming in ["referer", "authorization", "cookie", "proxy-authorization"] {
            assert!(
                !lowercase.contains(&format!("\r\n{incoming}:")),
                "the anonymous request carried {incoming}"
            );
        }
    }
}

#[tokio::test]
async fn rejects_unvalidated_redirects_and_stops_at_five_hops() {
    let fixture = Fixture::serve(vec![redirect("/again"); 6]).await;
    let client = ChannelIconClient::new().unwrap();
    assert_eq!(
        client.fetch(&fixture.url).await.err(),
        Some(ChannelIconError::RedirectRejected)
    );
    assert_eq!(fixture.finish().await.len(), 6);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = Fixture::serve(vec![
        redirect(&format!("http://credential:canary@{address}/image")),
        redirect(&format!("//@{address}/image")),
        redirect(&format!("/\\@{address}/image")),
        redirect("file:///icon.png"),
    ])
    .await;
    for _ in 0..4 {
        let error = client.fetch(&fixture.url).await.err();
        assert_eq!(error, Some(ChannelIconError::RedirectRejected));
    }
    assert_eq!(
        client
            .fetch(&format!("http://@{address}/image"))
            .await
            .err(),
        Some(ChannelIconError::InvalidLocation)
    );
    assert_eq!(fixture.finish().await.len(), 4);
    assert!(
        timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bounds_fixed_length_and_chunked_response_bodies() {
    let advertised =
        b"HTTP/1.1 200 OK\r\nContent-Length: 4194305\r\nConnection: close\r\n\r\n".to_vec();
    let mut chunked =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n400001\r\n"
            .to_vec();
    chunked.extend(std::iter::repeat_n(0, 4 * 1024 * 1024 + 1));
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    let fixture = Fixture::serve(vec![advertised, chunked]).await;
    let client = ChannelIconClient::new().unwrap();
    for _ in 0..2 {
        assert_eq!(
            client.fetch(&fixture.url).await.err(),
            Some(ChannelIconError::BodyTooLarge)
        );
    }
    assert_eq!(fixture.finish().await.len(), 2);
}

#[tokio::test]
async fn decodes_only_the_first_frame_and_checks_types_and_dimensions() {
    let mut gif = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut gif);
        for colour in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder
                .encode_frame(Frame::new(RgbaImage::from_pixel(1, 1, Rgba(colour))))
                .unwrap();
        }
    }
    let mut too_wide = gif.clone();
    too_wide[6..8].copy_from_slice(&4097_u16.to_le_bytes());
    let mut too_many_pixels = gif.clone();
    too_many_pixels[6..8].copy_from_slice(&4096_u16.to_le_bytes());
    too_many_pixels[8..10].copy_from_slice(&4096_u16.to_le_bytes());
    let mut too_wide_frame = gif.clone();
    let frame = gif.iter().position(|&byte| byte == 0x2c).unwrap();
    too_wide_frame[frame + 5..frame + 7].copy_from_slice(&4097_u16.to_le_bytes());

    let fixture = Fixture::serve(vec![
        response("200 OK", "", &gif),
        response("200 OK", "Content-Type: image/png\r\n", b"<svg/>"),
        response("200 OK", "", b"\x89PNG\r\n\x1a\n"),
        response("200 OK", "", &too_wide),
        response("200 OK", "", &too_many_pixels),
        response("200 OK", "", &too_wide_frame),
    ])
    .await;
    let client = ChannelIconClient::new().unwrap();
    let image = client.fetch(&fixture.url).await.unwrap().decode().unwrap();
    assert_eq!(image.source_dimensions(), (1, 1));
    assert_eq!(image.into_bgra(), (1, 1, vec![0, 0, 255, 255]));
    for error in [
        ChannelIconError::UnsupportedFormat,
        ChannelIconError::InvalidImage,
        ChannelIconError::ImageTooLarge,
        ChannelIconError::ImageTooLarge,
        ChannelIconError::ImageTooLarge,
    ] {
        let encoded = client.fetch(&fixture.url).await.unwrap();
        assert_eq!(encoded.decode().err(), Some(error));
    }
    assert_eq!(fixture.finish().await.len(), 6);
}

#[tokio::test]
async fn accepts_opaque_lossy_webp_and_falls_back_for_other_modes() {
    let mut lossless = Vec::new();
    WebPEncoder::new_lossless(&mut lossless)
        .write_image(&[255, 0, 0, 255], 1, 1, ExtendedColorType::Rgba8)
        .unwrap();
    let animated = animated_webp();
    let fixture = Fixture::serve(vec![
        response("200 OK", "", SIMPLE_WEBP),
        response("200 OK", "", &lossless),
        response("200 OK", "", COMPRESSED_ALPHA_WEBP),
        response("200 OK", "", &animated),
    ])
    .await;
    let client = ChannelIconClient::new().unwrap();
    let icon = client.fetch(&fixture.url).await.unwrap().decode().unwrap();
    assert_eq!(icon.source_dimensions(), (2, 2));
    let (width, height, bgra) = icon.into_bgra();
    assert_eq!((width, height, bgra.len()), (2, 2, 16));
    assert!(bgra.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
    let mut rejections = Vec::new();
    for _ in 0..3 {
        let encoded = client.fetch(&fixture.url).await.unwrap();
        rejections.push(encoded.decode().err());
    }
    assert_eq!(rejections, [Some(ChannelIconError::UnsupportedFormat); 3]);
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn rejects_oversized_or_non_simple_lossy_webp() {
    let mut oversized = SIMPLE_WEBP.to_vec();
    oversized[26..28].copy_from_slice(&4097_u16.to_le_bytes());

    let mut body = SIMPLE_WEBP[8..].to_vec();
    webp_chunk(&mut body, b"JUNK", &[0]);
    let mut extra_chunk = Vec::new();
    webp_chunk(&mut extra_chunk, b"RIFF", &body);

    let fixture = Fixture::serve(vec![
        response("200 OK", "", &oversized),
        response("200 OK", "", &extra_chunk),
    ])
    .await;
    let client = ChannelIconClient::new().unwrap();
    for error in [
        ChannelIconError::ImageTooLarge,
        ChannelIconError::InvalidImage,
    ] {
        let encoded = client.fetch(&fixture.url).await.unwrap();
        assert_eq!(encoded.decode().err(), Some(error));
    }
    assert_eq!(fixture.finish().await.len(), 2);
}

fn animated_webp() -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    webp_chunk(&mut body, b"VP8X", &[2, 0, 0, 0, 1, 0, 0, 1, 0, 0]);
    webp_chunk(&mut body, b"ANIM", &[0; 6]);
    let mut frame = vec![0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 100, 0, 0, 2];
    frame.extend_from_slice(&SIMPLE_WEBP[12..]);
    webp_chunk(&mut body, b"ANMF", &frame);
    let mut riff = Vec::new();
    webp_chunk(&mut riff, b"RIFF", &body);
    riff
}

fn webp_chunk(output: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(tag);
    output.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    output.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        output.push(0);
    }
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut result = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    result.extend_from_slice(body);
    result
}

fn redirect(location: &str) -> Vec<u8> {
    response(
        "302 Found",
        &format!("Location: {location}\r\nSet-Cookie: icon-fixture=not-forwarded; Path=/\r\n"),
        &[],
    )
}

struct Fixture {
    url: String,
    task: JoinHandle<()>,
    requests: mpsc::UnboundedReceiver<String>,
}

impl Fixture {
    async fn serve(replies: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (send, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0 && request.len() + count <= 16 * 1024);
                    request.extend_from_slice(&buffer[..count]);
                }
                send.send(String::from_utf8(request).unwrap()).unwrap();
                // The bounded reader can close the connection before receiving an oversized body.
                let _ = socket.write_all(&reply).await;
            }
        });
        Self {
            url,
            task,
            requests,
        }
    }

    async fn finish(mut self) -> Vec<String> {
        timeout(Duration::from_secs(1), self.task)
            .await
            .expect("all expected fixture requests arrived")
            .unwrap();
        let mut requests = Vec::new();
        while let Some(request) = self.requests.recv().await {
            requests.push(request);
        }
        requests
    }
}
