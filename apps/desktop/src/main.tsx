import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { AmbientApp } from "./AmbientApp";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

const isAmbientWindow = new URLSearchParams(window.location.search).get("window") === "ambient";
ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode>{isAmbientWindow ? <AmbientApp /> : <App />}</React.StrictMode>);
