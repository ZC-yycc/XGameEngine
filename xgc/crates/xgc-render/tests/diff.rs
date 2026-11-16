//! 图像差异指标。

use xgc_render::diff_images;
use xgc_render::ImageData;

fn image(pixels: [[u8; 4]; 4]) -> ImageData {
    ImageData {
        width: 2,
        height: 2,
        rgba: pixels.to_vec(),
    }
}

#[test]
fn reports_pixel_ratio_and_error() {
    let a = image([[0, 0, 0, 255]; 4]);
    let mut pixels = [[0, 0, 0, 255]; 4];
    pixels[1] = [100, 0, 0, 255];
    let b = image(pixels);
    let diff = diff_images(&a, &b, 0).unwrap();
    assert_eq!(diff.pixels, 4);
    assert_eq!(diff.different, 1);
    assert!((diff.ratio - 0.25).abs() < 1e-6);
    assert!(diff.mean_abs > 0.0);
    assert_eq!(diff.max_delta, 100);
}

#[test]
fn rejects_different_sizes() {
    let mut a = image([[0, 0, 0, 255]; 4]);
    let b = image([[0, 0, 0, 255]; 4]);
    a.width = 3;
    assert!(diff_images(&a, &b, 0).is_err());
}
