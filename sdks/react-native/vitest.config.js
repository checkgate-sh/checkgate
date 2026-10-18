import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

// Tests run in Node, where the `react-native` package isn't installed: resolve
// it to a stub whose NativeModules each test can control.
export default defineConfig({
  resolve: {
    alias: {
      'react-native': fileURLToPath(new URL('./test/react-native-stub.js', import.meta.url)),
    },
  },
})
