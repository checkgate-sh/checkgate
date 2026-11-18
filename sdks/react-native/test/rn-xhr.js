// The part of React Native's XMLHttpRequest that react-native-sse uses, over
// node:http: `responseText` grows as chunks arrive and `readystatechange`
// fires for every chunk while LOADING (as React Native's does; Node XHR
// polyfills buffer the body until it completes, which hides streaming).
import http from 'node:http'

export class ReactNativeXMLHttpRequest {
  static UNSENT = 0
  static OPENED = 1
  static HEADERS_RECEIVED = 2
  static LOADING = 3
  static DONE = 4

  constructor() {
    this.readyState = 0
    this.status = 0
    this.responseText = ''
    this.timeout = 0
    this.withCredentials = false
    this.onreadystatechange = null
    this.onerror = null
    this._headers = {}
    this._req = null
    this._aborted = false
  }

  open(method, url) {
    this._method = method
    this._url = url
    this._set(1)
  }

  setRequestHeader(name, value) { this._headers[name] = value }

  _set(state) {
    this.readyState = state
    if (typeof this.onreadystatechange === 'function') this.onreadystatechange()
  }

  send(body) {
    const req = http.request(this._url, { method: this._method, headers: this._headers }, (res) => {
      if (this._aborted) return res.destroy()
      this.status = res.statusCode
      this._set(2)
      res.setEncoding('utf8')
      res.on('data', (chunk) => {
        if (this._aborted) return
        this.responseText += chunk
        this._set(3)
      })
      res.on('end', () => { if (!this._aborted) this._set(4) })
      res.on('error', () => this._fail())
      res.on('aborted', () => this._fail())
    })
    req.on('error', () => this._fail())
    this._req = req
    if (body) req.write(body)
    req.end()
  }

  _fail() {
    if (this._aborted || this.readyState === 4) return
    this.readyState = 4
    this.status = 0
    if (typeof this.onerror === 'function') this.onerror()
  }

  abort() {
    this._aborted = true
    if (this._req) this._req.destroy()
  }
}
