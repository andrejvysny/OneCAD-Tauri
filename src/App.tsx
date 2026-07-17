import DevGallery from "@/app/DevGallery";
import { StartScreen } from "@/features/start/StartScreen";
import { EditorScreen } from "@/features/shell/EditorScreen";
import { useAppStore } from "@/stores/appStore";

/**
 * App shell: switches between the start screen and the (placeholder) editor.
 * `?gallery` still mounts the F-WP1 primitive/icon showcase (DevGallery).
 */
function App() {
  const screen = useAppStore((s) => s.screen);
  const params = new URLSearchParams(window.location.search);
  const showGallery = params.has("gallery");
  // Viewport/sketch demos live in the editor shell — boot straight into it so
  // Playwright can exercise them without the start-screen click-through.
  const forceEditor = params.has("vpdemo") || params.has("sketchdemo") || params.has("toolsdemo");

  if (showGallery) {
    return <DevGallery />;
  }

  return screen === "start" && !forceEditor ? <StartScreen /> : <EditorScreen />;
}

export default App;
