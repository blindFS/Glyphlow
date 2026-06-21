use block2::RcBlock;
use objc2::{
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath, CGPath};
use objc2_foundation::{NSArray, NSNumber, ns_string};
use objc2_quartz_core::{
    CAAnimation, CAAnimationGroup, CABasicAnimation, CAGradientLayer, CAMediaTiming,
    CAMediaTimingFunction, CAShapeLayer, CATransaction, kCAMediaTimingFunctionEaseOut,
};

use crate::user_interface::{HintBox, UIDrawer, calibrated_origin};

const RIPPLE_DURATION: f64 = 0.4;
const RIPPLE_INIT_RADIUS: f64 = 5.0;
const RIPPLE_SCALE_FACTOR: f64 = 5.0;

const TRAIL_DURATION: f64 = 0.5;
/// Offset from the ending cursor position to the right-bottom corner of the cursor shape
const CURSOR_OFFSET_X: f64 = 9.0;
const CURSOR_OFFSET_Y: f64 = 15.0;

const HINT_FADE_OUT_DURATION: f64 = 0.5;

impl UIDrawer {
    /// Triggers a ripple animation at the given (x, y) coordinates inside a parent CALayer.
    pub fn draw_ripple(&self, x: f64, y: f64, color: &CFRetained<CGColor>) {
        autoreleasepool(|_| {
            let initial_radius = RIPPLE_INIT_RADIUS;

            let ripple_layer = CAShapeLayer::new();

            // Position the layer at the click location and define bounds centered at (0, 0)
            ripple_layer.setPosition(calibrated_origin(x, y, &self.overlay_frame));
            ripple_layer.setBounds(CGRect::new(
                CGPoint::new(-initial_radius, -initial_radius),
                CGSize::new(initial_radius * 2.0, initial_radius * 2.0),
            ));

            // Create a circular path centered at (0, 0) inside the layer's local space
            let circle_bounds = CGRect::new(
                CGPoint::new(-initial_radius, -initial_radius),
                CGSize::new(initial_radius * 2.0, initial_radius * 2.0),
            );
            let circle_path =
                unsafe { CGPath::with_ellipse_in_rect(circle_bounds, std::ptr::null()) };

            ripple_layer.setPath(Some(&circle_path));
            ripple_layer.setFillColor(Some(color));
            ripple_layer.setOpacity(0.0); // Set model opacity to 0.0 (final state) to prevent flicker

            // Ensure the layer scales from its center point
            ripple_layer.setAnchorPoint(CGPoint::new(0.5, 0.5));

            self.root.addSublayer(&ripple_layer);

            unsafe {
                CATransaction::begin();

                // Use CATransaction completion block to remove the layer after the animation finishes
                let layer_to_remove = ripple_layer.clone();
                let completion_block = RcBlock::new(move || {
                    layer_to_remove.removeFromSuperlayer();
                });
                CATransaction::setCompletionBlock(Some(&completion_block));

                // Create the Scale Animation (Expanding)
                let scale_anim =
                    CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.scale")));
                scale_anim.setFromValue(Some(&NSNumber::new_f64(1.0)));
                scale_anim.setToValue(Some(&NSNumber::new_f64(RIPPLE_SCALE_FACTOR))); // Grows 6x the initial radius

                // Create the Opacity Animation (Fading Out)
                let fade_anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
                fade_anim.setFromValue(Some(&NSNumber::new_f64(1.0)));
                fade_anim.setToValue(Some(&NSNumber::new_f64(0.0)));

                // Group the animations together
                let anim_group = CAAnimationGroup::animation();
                let scale_anim: Retained<CAAnimation> = Retained::cast_unchecked(scale_anim);
                let fade_anim: Retained<CAAnimation> = Retained::cast_unchecked(fade_anim);
                let animations = NSArray::from_retained_slice(&[scale_anim, fade_anim]);

                anim_group.setAnimations(Some(&animations));
                anim_group.setDuration(RIPPLE_DURATION);

                // Smooth out the animation speed curve
                let timing_function =
                    CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseOut);
                anim_group.setTimingFunction(Some(&timing_function));

                // Tells Core Animation to automatically delete the animation once it finishes
                anim_group.setRemovedOnCompletion(true);

                // Bind animation to layer and add layer to window/view hierarchy
                // The key "ripple" can be anything unique
                ripple_layer.addAnimation_forKey(&anim_group, Some(ns_string!("ripple")));

                CATransaction::commit();
            }
        });
    }

    /// Draws a fading-out triangle cursor trail from `(start_x, start_y)` to `(end_x, end_y)`.
    /// Ending coordinates are in screen coordinates (top-left origin), need calibration before drawing
    /// while start coordinates are in bottom-left origin, but relative to the main screen origin.
    ///
    /// The triangle vertices are:
    ///   1. Starting cursor position
    ///   2. Ending cursor position
    ///   3. Right-bottom corner of the ending cursor shape (constant offset from ending position)
    pub fn draw_trail(&self, start_x: f64, start_y: f64, end_x: f64, end_y: f64) {
        let (start_x, start_y) = if self.screen_frames.len() > 1 {
            let main_frame = self.screen_frames[0];
            let start_x = start_x + main_frame.top_left.x - self.overlay_frame.top_left.x;
            let start_y = start_y - main_frame.bottom_right.y + self.overlay_frame.bottom_right.y;
            (start_x, start_y)
        } else {
            (start_x, start_y)
        };
        // Convert ending point from top-left origin to bottom-left origin (Cocoa/CALayer coordinates)
        let CGPoint { x: end_x, y: end_y } = calibrated_origin(end_x, end_y, &self.overlay_frame);
        // Skip if start and end are the same (no movement)
        if (start_x - end_x).abs() < 1.0 && (start_y - end_y).abs() < 1.0 {
            return;
        }

        autoreleasepool(|_| {
            // Right-bottom corner of cursor shape at ending position
            let cx = end_x + CURSOR_OFFSET_X;
            let cy = end_y - CURSOR_OFFSET_Y;

            // Calculate bounding box for the triangle
            let min_x = start_x.min(end_x).min(cx);
            let max_x = start_x.max(end_x).max(cx);
            let min_y = start_y.min(end_y).min(cy);
            let max_y = start_y.max(end_y).max(cy);

            let width = (max_x - min_x).max(1.0);
            let height = (max_y - min_y).max(1.0);

            // Build the triangle path relative to the bounding box
            let path = CGMutablePath::new();
            unsafe {
                CGMutablePath::move_to_point(
                    Some(&path),
                    std::ptr::null(),
                    start_x - min_x,
                    start_y - min_y,
                );
                CGMutablePath::add_line_to_point(
                    Some(&path),
                    std::ptr::null(),
                    end_x - min_x,
                    end_y - min_y,
                );
                CGMutablePath::add_line_to_point(
                    Some(&path),
                    std::ptr::null(),
                    cx - min_x,
                    cy - min_y,
                );
                CGMutablePath::close_subpath(Some(&path));
            }

            let trail_layer = CAShapeLayer::new();
            trail_layer.setFrame(CGRect::new(
                CGPoint::new(min_x, min_y),
                CGSize::new(width, height),
            ));
            trail_layer.setPath(Some(&path));
            trail_layer.setFillColor(Some(&self.theme.hint_bg_color));
            trail_layer.setOpacity(1.0); // Keep opaque, let mask handle transparency

            self.root.addSublayer(&trail_layer);

            // Create a gradient mask to achieve spatial fade
            let mask_layer = CAGradientLayer::new();
            mask_layer.setFrame(CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width, height),
            ));

            // Gradient from start point to end point
            mask_layer.setStartPoint(CGPoint::new(
                (start_x - min_x) / width,
                (start_y - min_y) / height,
            ));
            mask_layer.setEndPoint(CGPoint::new(
                (end_x - min_x) / width,
                (end_y - min_y) / height,
            ));

            unsafe {
                // Set colors for the mask (transparent to opaque)
                let clear_color = CGColor::new_generic_rgb(1.0, 1.0, 1.0, 0.0);
                let opaque_color = CGColor::new_generic_rgb(1.0, 1.0, 1.0, 1.0);

                // We need to cast CGColor to AnyObject to put it in NSArray
                // This is safe because CGColor is a CFType and CFType is bridged to AnyObject/id on macOS
                let clear_obj: Retained<AnyObject> =
                    Retained::retain(CFRetained::as_ptr(&clear_color).as_ptr() as *mut AnyObject)
                        .expect("Failed to retain CGColor");
                let opaque_obj: Retained<AnyObject> =
                    Retained::retain(CFRetained::as_ptr(&opaque_color).as_ptr() as *mut AnyObject)
                        .expect("Failed to retain CGColor");

                let colors = NSArray::from_retained_slice(&[clear_obj, opaque_obj]);
                mask_layer.setColors(Some(&colors));
                trail_layer.setMask(Some(&mask_layer));

                CATransaction::begin();

                let layer_to_remove = trail_layer.clone();
                let completion_block = RcBlock::new(move || {
                    layer_to_remove.removeFromSuperlayer();
                });
                CATransaction::setCompletionBlock(Some(&completion_block));

                // Animate locations of the gradient mask to create the spatial fade effect.
                // At t=0, locations are [-1.0, 0.0], meaning the entire range [0, 1] is opaque.
                // At t=1, locations are [1.0, 2.0], meaning the entire range [0, 1] is transparent.
                // This results in the "fade front" moving from the start point to the end point.
                let loc_anim =
                    CABasicAnimation::animationWithKeyPath(Some(ns_string!("locations")));
                let loc_start = NSArray::from_retained_slice(&[
                    NSNumber::new_f64(-0.2),
                    NSNumber::new_f64(0.0),
                ]);
                let loc_end =
                    NSArray::from_retained_slice(&[NSNumber::new_f64(1.0), NSNumber::new_f64(1.2)]);
                loc_anim.setFromValue(Some(&loc_start));
                loc_anim.setToValue(Some(&loc_end));
                loc_anim.setDuration(TRAIL_DURATION);
                loc_anim.setRemovedOnCompletion(true);

                let timing_function =
                    CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionEaseOut);
                loc_anim.setTimingFunction(Some(&timing_function));

                mask_layer.addAnimation_forKey(&loc_anim, Some(ns_string!("locations")));
                // Set the model's locations to the final value to prevent flickering after animation
                mask_layer.setLocations(Some(&loc_end));

                CATransaction::commit();
            }
        });
    }
}

impl HintBox {
    /// Fades the hint box out, then hides it.
    pub fn fade_out(&self, hide_box: bool) {
        let mut layers = self.frame_layer.iter().collect::<Vec<_>>();
        let mut ending_opacity = 0.25;
        if hide_box {
            layers.push(&self.box_layer);
            ending_opacity = 0.0;
        }

        for layer in layers {
            // Set the model opacity to 0 before the animation so the layer
            // stays hidden after the animation is removed.
            layer.setOpacity(ending_opacity);

            CATransaction::begin();
            unsafe {
                let anim = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
                anim.setFromValue(Some(&NSNumber::new_f32(1.0)));
                anim.setToValue(Some(&NSNumber::new_f32(ending_opacity)));
                anim.setDuration(HINT_FADE_OUT_DURATION);
                anim.setRemovedOnCompletion(true);
                layer.addAnimation_forKey(&anim, Some(ns_string!("fade_out")));
            }
            CATransaction::commit();
        }
    }
}
