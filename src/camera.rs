//! Orbit camera with inertia. Always looks at the globe center.
#![allow(deprecated)] // glam 0.33 deprecates Mat4::look_at_rh/perspective_rh; fine here.

use glam::{Mat4, Vec3};

pub struct OrbitCamera {
    pub lat: f32,
    pub lon: f32,
    pub dist: f32,
    vel_lat: f32,
    vel_lon: f32,
    zoom_vel: f32,
    /// Smoothed measure of recent camera motion, 0..1 (drives trail fading).
    pub motion: f32,
    pub auto_orbit: bool,
    idle_secs: f32,
}

const DIST_MIN: f32 = 1.18;
const DIST_MAX: f32 = 20.0;

impl OrbitCamera {
    pub fn new() -> Self {
        Self {
            lat: 26.0,
            lon: 136.0,
            dist: 3.3,
            vel_lat: 0.0,
            vel_lon: 0.0,
            zoom_vel: 0.0,
            motion: 0.0,
            auto_orbit: true,
            idle_secs: 100.0,
        }
    }

    pub fn reset(&mut self) {
        self.lat = 26.0;
        self.lon = 136.0;
        self.dist = 3.3;
        self.vel_lat = 0.0;
        self.vel_lon = 0.0;
        self.zoom_vel = 0.0;
    }

    /// drag in points, scroll in points, pinch as multiplicative factor.
    pub fn update(&mut self, dt: f32, drag: (f32, f32), scroll: f32, pinch: f32, view_h: f32) {
        let dt = dt.clamp(0.0005, 0.05);
        // Sensitivity shrinks as we zoom in.
        let fov_scale = ((self.dist - 1.0) / 2.3).clamp(0.02, 3.0);
        let k = 80.0 * fov_scale / view_h.max(1.0);

        if drag.0 != 0.0 || drag.1 != 0.0 {
            self.vel_lon = -drag.0 * k / dt.max(1e-3) * 0.016;
            self.vel_lat = drag.1 * k / dt.max(1e-3) * 0.016;
            self.idle_secs = 0.0;
        }
        if scroll.abs() > 0.0 {
            self.zoom_vel -= scroll * 0.0022;
            self.idle_secs = 0.0;
        }
        if (pinch - 1.0).abs() > 1e-4 {
            self.zoom_vel -= (pinch - 1.0) * 1.4;
            self.idle_secs = 0.0;
        }

        self.lon += self.vel_lon * dt * 60.0 * 0.14;
        self.lat += self.vel_lat * dt * 60.0 * 0.14;
        self.dist *= 1.0 + self.zoom_vel * dt * 60.0 * 0.05;

        let decay = (-dt * 5.0).exp();
        self.vel_lon *= decay;
        self.vel_lat *= decay;
        self.zoom_vel *= (-dt * 7.0).exp();

        self.idle_secs += dt;
        if self.auto_orbit && self.idle_secs > 6.0 {
            self.lon += 0.55 * dt;
        }

        self.lat = self.lat.clamp(-85.0, 85.0);
        self.lon = self.lon.rem_euclid(360.0);
        self.dist = self.dist.clamp(DIST_MIN, DIST_MAX);

        let raw = (self.vel_lon.abs() + self.vel_lat.abs()) * 0.25 + self.zoom_vel.abs() * 12.0;
        let target = raw.min(1.0);
        let blend = 1.0 - (-dt * 8.0).exp();
        self.motion += (target - self.motion) * blend;
    }

    pub fn eye(&self) -> Vec3 {
        crate::astro::latlon_to_world(self.lat, self.lon) * self.dist
    }

    /// Returns (view_proj, inv_view_proj, eye).
    pub fn matrices(&self, aspect: f32) -> (Mat4, Mat4, Vec3) {
        let eye = self.eye();
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        // Narrow the FOV a touch when zoomed way in for a more "telephoto" feel.
        let fov = 42.0_f32.to_radians();
        let proj = Mat4::perspective_rh(fov, aspect.max(0.1), 0.02, 80.0);
        let vp = proj * view;
        (vp, vp.inverse(), eye)
    }
}
