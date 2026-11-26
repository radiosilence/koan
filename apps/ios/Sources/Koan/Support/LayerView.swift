import UIKit

/// A view whose drawing is its own sublayers, which keeps motion in the render
/// server rather than on the main thread.
///
/// Subclasses override `layoutLayers`, which runs on every layout pass and
/// which they call themselves to place their layers at once, and
/// `appearanceChanged`, for anything holding a resolved `CGColor`.
class LayerView: UIView {
    override init(frame: CGRect) {
        super.init(frame: frame)
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: LayerView, _) in
            view.appearanceChanged()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    /// The size may have changed.
    func layoutLayers() {}

    /// Light and dark swapped; anything holding a resolved `CGColor` repaints.
    func appearanceChanged() {}

    /// A dynamic colour pinned to this view's current appearance.
    func resolved(_ colour: UIColor) -> CGColor {
        colour.resolvedColor(with: traitCollection).cgColor
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        layoutLayers()
    }
}
