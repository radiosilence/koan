import SwiftUI

/// The app icon's ensō, as a vector.
///
/// Traced from `AppIcon.svg` rather than approximated, so the placeholder is
/// the same mark as the icon rather than a circle that looks vaguely like it.
/// Coordinates are in the icon's 512pt space and scaled to fit whatever they
/// are drawn into.
///
/// Thinned from 401 points to every fourth: the curve is smooth, and nothing
/// drawn at icon size can show the difference.
struct EnsoShape: Shape {
    /// Icon-space coordinates. Generated — see `just macos-icon` for the source.
    private static let points: [(CGFloat, CGFloat)] = [
        (230.3, 401.8),
        (222.0, 399.7),
        (213.9, 397.3),
        (205.8, 394.6),
        (198.0, 391.4),
        (190.3, 387.7),
        (182.8, 383.8),
        (175.2, 380.0),
        (167.3, 376.2),
        (159.4, 372.4),
        (151.7, 367.8),
        (145.0, 362.1),
        (139.7, 355.0),
        (135.7, 347.0),
        (132.6, 338.6),
        (129.7, 330.4),
        (126.3, 322.6),
        (123.0, 314.9),
        (120.2, 307.1),
        (118.3, 298.9),
        (117.2, 290.6),
        (116.3, 282.4),
        (114.8, 274.3),
        (112.2, 266.3),
        (108.8, 258.1),
        (105.7, 249.4),
        (103.9, 240.6),
        (104.0, 231.7),
        (106.1, 223.0),
        (109.3, 214.8),
        (113.0, 206.8),
        (116.6, 198.8),
        (120.4, 191.0),
        (124.8, 183.6),
        (130.3, 176.8),
        (136.9, 171.1),
        (144.2, 166.1),
        (151.1, 161.2),
        (157.2, 155.8),
        (162.4, 149.5),
        (167.2, 142.4),
        (172.4, 135.3),
        (178.4, 128.8),
        (185.3, 123.6),
        (192.9, 119.4),
        (200.8, 115.8),
        (208.7, 112.1),
        (216.8, 108.5),
        (225.1, 105.4),
        (233.8, 103.8),
        (242.7, 104.1),
        (251.6, 106.4),
        (260.1, 109.7),
        (268.3, 112.9),
        (276.3, 115.2),
        (284.4, 116.6),
        (292.6, 117.4),
        (300.9, 118.6),
        (309.0, 120.8),
        (316.8, 123.8),
        (324.5, 127.2),
        (332.4, 130.4),
        (340.6, 133.3),
        (349.2, 136.2),
        (357.5, 140.0),
        (364.8, 145.2),
        (370.6, 152.1),
        (374.7, 160.2),
        (377.8, 168.8),
        (380.7, 177.2),
        (383.8, 185.2),
        (387.2, 192.8),
        (390.4, 200.6),
        (392.8, 208.6),
        (394.3, 216.9),
        (395.1, 225.2),
        (396.2, 233.3),
        (398.3, 241.3),
        (401.3, 249.4),
        (404.7, 257.8),
        (407.4, 266.6),
        (408.3, 275.5),
        (407.2, 284.3),
        (404.5, 292.8),
        (401.0, 300.9),
        (397.3, 308.8),
        (393.7, 316.7),
        (389.6, 324.4),
        (384.7, 331.5),
        (378.9, 337.9),
        (372.5, 343.8),
        (366.2, 349.4),
        (360.3, 355.3),
        (354.8, 361.6),
        (349.3, 368.0),
        (343.5, 374.2),
        (337.0, 379.8),
        (329.9, 384.6),
        (322.4, 388.6),
        (314.6, 392.1),
        (306.6, 395.1)
    ]

    private static let designSize: CGFloat = 512

    func path(in rect: CGRect) -> Path {
        let scale = min(rect.width, rect.height) / Self.designSize
        let offsetX = rect.midX - (Self.designSize * scale) / 2
        let offsetY = rect.midY - (Self.designSize * scale) / 2

        var path = Path()
        for (index, point) in Self.points.enumerated() {
            let p = CGPoint(x: offsetX + point.0 * scale, y: offsetY + point.1 * scale)
            if index == 0 { path.move(to: p) } else { path.addLine(to: p) }
        }
        return path
    }
}
