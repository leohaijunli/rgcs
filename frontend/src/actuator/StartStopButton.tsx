import { useCallback, useEffect, useRef, useState, type PointerEvent } from 'react'

interface StartStopButtonProps {
  running: boolean
  pending?: boolean
  disabled: boolean
  holdMs: number
  title: string
  highlight?: boolean
  onStart: () => Promise<void>
  onStop: () => Promise<void>
}

/** One gesture has one meaning even when START changes to STOP mid-gesture. */
export default function StartStopButton(props: StartStopButtonProps) {
  const current = useRef(props)
  current.current = props
  const timer = useRef<number>()
  const suppressReleaseClick = useRef(false)
  const inFlight = useRef(false)
  const [starting, setStarting] = useState(false)
  const [progress, setProgress] = useState(0)

  const cancelHold = useCallback(() => {
    if (timer.current !== undefined) {
      cancelAnimationFrame(timer.current)
      timer.current = undefined
    }
    setProgress(0)
  }, [])

  const start = async () => {
    if (inFlight.current || current.current.disabled || current.current.running || current.current.pending) return
    inFlight.current = true
    setStarting(true)
    try {
      await current.current.onStart()
    } finally {
      inFlight.current = false
      setStarting(false)
    }
  }

  const pointerDown = (event: PointerEvent<HTMLButtonElement>) => {
    // A new STOP gesture is distinct from releasing the preceding START hold.
    suppressReleaseClick.current = false
    if (event.button !== 0 || props.running || props.pending || props.disabled || starting || props.holdMs === 0) return
    suppressReleaseClick.current = true
    event.currentTarget.setPointerCapture(event.pointerId)
    const started = performance.now()
    const step = () => {
      if (current.current.disabled || current.current.running) {
        cancelHold()
        return
      }
      const next = Math.min(1, (performance.now() - started) / current.current.holdMs)
      setProgress(next)
      if (next >= 1) {
        timer.current = undefined
        setProgress(0)
        void start()
      } else {
        timer.current = requestAnimationFrame(step)
      }
    }
    timer.current = requestAnimationFrame(step)
  }

  useEffect(() => {
    cancelHold()
  }, [props.disabled, props.running, props.holdMs, cancelHold])
  useEffect(() => () => {
    if (timer.current !== undefined) cancelAnimationFrame(timer.current)
  }, [])

  return (
    <button
      className={`start-btn${props.running ? ' stop' : ''}${props.highlight ? ' guide-target' : ''}`}
      style={{ marginTop: 8, position: 'relative' }}
      disabled={!props.running && !starting && !props.pending && props.disabled}
      title={props.running ? 'Stop Motor Test and release its owned outputs. Manual recording continues.' : starting || props.pending ? 'Cancel the pending start; an already dispatched short tone may finish, but motors will not start.' : props.title}
      onPointerDown={pointerDown}
      onPointerUp={cancelHold}
      onPointerCancel={cancelHold}
      onLostPointerCapture={cancelHold}
      onPointerMove={(event) => {
        if (timer.current === undefined) return
        const bounds = event.currentTarget.getBoundingClientRect()
        if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) cancelHold()
      }}
      onClick={() => {
        if (suppressReleaseClick.current) {
          suppressReleaseClick.current = false
          return
        }
        if (props.running || starting || props.pending) {
          void props.onStop()
        } else if (props.holdMs === 0) {
          void start()
        }
      }}
    >
      {props.running ? 'STOP' : starting || props.pending ? 'CANCEL START' : 'START'}
      {progress > 0 && <span className="hold-fill" style={{ width: `${progress * 100}%` }} aria-hidden />}
    </button>
  )
}
