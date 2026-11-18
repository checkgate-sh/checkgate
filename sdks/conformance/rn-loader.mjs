// Node module hook for running the React Native SDK on a host: `react-native`
// resolves to the stub the unit tests use (only NativeModules is imported, and
// the JSI global is installed directly).
const stub = new URL('../react-native/test/react-native-stub.js', import.meta.url).href;

export async function resolve(specifier, context, next) {
    if (specifier === 'react-native') return { url: stub, shortCircuit: true };
    return next(specifier, context);
}
