import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import { router } from './router';
import { registerPlugins } from './plugins';
import './styles/tokens.css';
import './styles/base.css';

// Bootstrap. Plugin registration happens BEFORE mount so plugin-registered
// routes + rail entries land in the router and the rail config before the
// first navigation. Plugins consume `window.__LVQR_ADMIN_PLUGINS__`; see
// `src/plugins/index.ts`.
const app = createApp(App);
app.use(createPinia());
app.use(router);
registerPlugins(router);
app.mount('#app');
