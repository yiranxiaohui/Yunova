/// <reference types="@capacitor/cli" />
import type { CapacitorConfig } from "@capacitor/cli"

/**
 * Capacitor packaging for the mobile remote.
 *
 * The phone is a remote control, not an execution target: it drives tasks that
 * run in a cloud sandbox or on a paired computer. So this ships the same React
 * bundle the browser uses and adds only what a browser cannot do — chiefly
 * notifying the user that an agent is blocked waiting for approval.
 *
 * `webDir` points at the existing Vite output, so there is no separate mobile
 * build to keep in step.
 */
const config: CapacitorConfig = {
  appId: "top.yunnet.yunova",
  appName: "Yunova",
  webDir: "dist",
  // Cleartext traffic is off, so a self-hosted instance must be served over
  // HTTPS. Allowing it would mean a session cookie and a pairing code could
  // cross the network in the clear.
  server: {
    androidScheme: "https",
    iosScheme: "https",
  },
  ios: {
    // The WebView draws under the status bar so env(safe-area-inset-*) works;
    // the layout pads for it via .safe-top / .safe-bottom.
    contentInset: "never",
  },
  android: {
    // Release builds must not accept arbitrary certificates.
    allowMixedContent: false,
  },
  plugins: {
    // Approval requests arrive over a socket this client already holds, so a
    // local notification is enough — no server-side push infrastructure.
    LocalNotifications: {
      smallIcon: "ic_stat_icon",
      iconColor: "#6d3bd1",
    },
    SplashScreen: {
      launchAutoHide: true,
      backgroundColor: "#ffffff",
    },
  },
}

export default config
