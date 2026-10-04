// The renderer's only door to Electron (frontend/src/bridge.ts). Business data never passes here:
// the renderer talks to the backend directly (frontend.md §1).
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('lobotomy', {
  connection: () => ipcRenderer.invoke('lobotomy:connection'),
  chooseRepo: () => ipcRenderer.invoke('lobotomy:chooseRepo'),
  setAttention: (count) => ipcRenderer.send('lobotomy:attention', count),
  openExternal: (url) => ipcRenderer.send('lobotomy:openExternal', url),
});
