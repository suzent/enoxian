import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

// jsdom implements no scrolling, so `Element.scrollTo` is simply absent and any
// component that scrolls a list throws from inside a rAF callback — which
// surfaces as an unhandled error that fails the run *after* the assertions
// passed. Stub it rather than making every such test defend itself.
if (!Element.prototype.scrollTo) {
  Element.prototype.scrollTo = () => {}
}
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {}
}

afterEach(cleanup)
