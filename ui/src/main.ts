import "@fontsource/schibsted-grotesk/400.css";
import "@fontsource/schibsted-grotesk/500.css";
import "@fontsource/schibsted-grotesk/600.css";
import "@fontsource/schibsted-grotesk/700.css";
import "@fontsource/martian-mono/400.css";
import "@fontsource/martian-mono/500.css";
import "./style.css";

import { mount } from "./app";
import { createBridge } from "./bridge";

const root = document.getElementById("app");
if (!root) {
  throw new Error("Cordial could not find its mount point");
}

mount(root, createBridge());
