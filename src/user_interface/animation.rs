use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGPath};
use objc2_foundation::{NSNumber, NSString};
use objc2_quartz_core::{
    CAAnimation, CAAnimationGroup, CABasicAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction,
    CAShapeLayer, CATransaction, kCAMediaTimingFunctionEaseOut,
};

const RIPPLE_DURATION: f64 = 0.4;
const RIPPLE_INIT_RADIUS: f64 = 5.0;
const RIPPLE_SCALE_FACTOR: f64 = 5.0;

/// Triggers a ripple animation at the given (x, y) coordinates inside a parent CALayer.
pub fn draw_ripple(parent_layer: &CALayer, x: f64, y: f64, color: &CFRetained<CGColor>) {
    autoreleasepool(|_| {
        let initial_radius = RIPPLE_INIT_RADIUS;

        let ripple_layer = CAShapeLayer::new();

        // Position the layer at the click location and define bounds centered at (0, 0)
        ripple_layer.setPosition(CGPoint::new(x, y));
        ripple_layer.setBounds(CGRect::new(
            CGPoint::new(-initial_radius, -initial_radius),
            CGSize::new(initial_radius * 2.0, initial_radius * 2.0),
        ));

        // Create a circular path centered at (0, 0) inside the layer's local space
        let circle_bounds = CGRect::new(
            CGPoint::new(-initial_radius, -initial_radius),
            CGSize::new(initial_radius * 2.0, initial_radius * 2.0),
        );
        let circle_path = unsafe { CGPath::with_ellipse_in_rect(circle_bounds, std::ptr::null()) };

        ripple_layer.setPath(Some(&circle_path));
        ripple_layer.setFillColor(Some(color));
        ripple_layer.setOpacity(0.0); // Set model opacity to 0.0 (final state) to prevent flicker

        // Ensure the layer scales from its center point
        ripple_layer.setAnchorPoint(CGPoint::new(0.5, 0.5));

        parent_layer.addSublayer(&ripple_layer);

        unsafe {
            CATransaction::begin();

            // Use CATransaction completion block to remove the layer after the animation finishes
            let layer_to_remove = ripple_layer.clone();
            let completion_block = RcBlock::new(move || {
                layer_to_remove.removeFromSuperlayer();
            });
            CATransaction::setCompletionBlock(Some(&completion_block));

            // Create the Scale Animation (Expanding)
            let scale_anim = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(
                "transform.scale",
            )));
            scale_anim.setFromValue(Some(&NSNumber::new_f64(1.0)));
            scale_anim.setToValue(Some(&NSNumber::new_f64(RIPPLE_SCALE_FACTOR))); // Grows 6x the initial radius

            // Create the Opacity Animation (Fading Out)
            let fade_anim =
                CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
            fade_anim.setFromValue(Some(&NSNumber::new_f64(1.0)));
            fade_anim.setToValue(Some(&NSNumber::new_f64(0.0)));

            // Group the animations together
            let anim_group = CAAnimationGroup::animation();
            let scale_anim: Retained<CAAnimation> = Retained::cast_unchecked(scale_anim);
            let fade_anim: Retained<CAAnimation> = Retained::cast_unchecked(fade_anim);
            let animations =
                objc2_foundation::NSArray::from_retained_slice(&[scale_anim, fade_anim]);

            anim_group.setAnimations(Some(&animations));
            anim_group.setDuration(RIPPLE_DURATION); // 400 milliseconds (0.4 seconds)

            // Smooth out the animation speed curve
            let timing_function =
                CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseOut);
            anim_group.setTimingFunction(Some(&timing_function));

            // Tells Core Animation to automatically delete the animation once it finishes
            anim_group.setRemovedOnCompletion(true);

            // Bind animation to layer and add layer to window/view hierarchy
            // The key "ripple" can be anything unique
            ripple_layer.addAnimation_forKey(&anim_group, Some(&NSString::from_str("ripple")));

            CATransaction::commit();
        }
    });
}
