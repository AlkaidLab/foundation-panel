// The page of the full-screen PIN window (src-tauri/src/pin_window.rs).
import '../../../src-tauri/inject-traces.js'  // no trace of a browser: drag ghosts, browser keys and menus, zoom, selected chrome
import { createApp } from 'vue'
import 'foundation-ui/all.css'  // Material 3 Expressive tokens and components
import PinApp from './PinApp.vue'

createApp(PinApp).mount('#app')
