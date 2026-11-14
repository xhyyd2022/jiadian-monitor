import { createApp } from "vue";
import ElementPlus from "element-plus";
import zhCn from "element-plus/es/locale/lang/zh-cn";
import "element-plus/dist/index.css";
import "element-plus/theme-chalk/dark/css-vars.css";
import App from "./App.vue";

createApp(App)
  .use(ElementPlus, {
    locale: zhCn,
    size: "small",
    zIndex: 3000,
  })
  .mount("#app");
