package main

import "sort"

// order is a single order. Resting orders are linked into their price level's FIFO.
type order struct {
	id       int64
	acct     int64
	sym      int
	buy      bool
	market   bool
	qty      int64
	px       int64 // 0 for market orders
	pReserve int64 // per-share frozen cash for buys (px, or last_at_entry*2 for market)
	filled   int64
	ts       int64

	lvl        *level
	prev, next *order
}

func (o *order) remaining() int64 { return o.qty - o.filled }

// level is a FIFO of resting orders at one price, with aggregated open qty.
type level struct {
	px         int64
	total      int64
	head, tail *order
}

func (l *level) push(o *order) {
	o.lvl = l
	o.prev = l.tail
	o.next = nil
	if l.tail != nil {
		l.tail.next = o
	} else {
		l.head = o
	}
	l.tail = o
	l.total += o.remaining()
}

func (l *level) unlink(o *order) {
	if o.prev != nil {
		o.prev.next = o.next
	} else {
		l.head = o.next
	}
	if o.next != nil {
		o.next.prev = o.prev
	} else {
		l.tail = o.prev
	}
	o.prev, o.next, o.lvl = nil, nil, nil
}

// side holds sorted price levels with the BEST level at the end of the slice,
// so consuming the best level is a pop.
//
//	bids: ascending price (best = highest = last)
//	asks: descending price (best = lowest = last)
type side struct {
	isBid  bool
	levels []*level
}

func (s *side) best() *level {
	if len(s.levels) == 0 {
		return nil
	}
	return s.levels[len(s.levels)-1]
}

// find returns the index where a level with price px is or would be inserted.
func (s *side) find(px int64) int {
	if s.isBid {
		return sort.Search(len(s.levels), func(i int) bool { return s.levels[i].px >= px })
	}
	return sort.Search(len(s.levels), func(i int) bool { return s.levels[i].px <= px })
}

func (s *side) add(o *order) {
	i := s.find(o.px)
	if i < len(s.levels) && s.levels[i].px == o.px {
		s.levels[i].push(o)
		return
	}
	l := &level{px: o.px}
	l.push(o)
	s.levels = append(s.levels, nil)
	copy(s.levels[i+1:], s.levels[i:])
	s.levels[i] = l
}

// remove unlinks o from its level (whose total must already exclude o's
// remaining qty) and deletes the level if it became empty.
func (s *side) remove(o *order) {
	l := o.lvl
	l.unlink(o)
	if l.head != nil {
		return
	}
	n := len(s.levels)
	if n > 0 && s.levels[n-1] == l {
		s.levels[n-1] = nil
		s.levels = s.levels[:n-1]
		return
	}
	i := s.find(l.px)
	if i < n && s.levels[i] == l {
		copy(s.levels[i:], s.levels[i+1:])
		s.levels[n-1] = nil
		s.levels = s.levels[:n-1]
	}
}

// depth returns up to n [px, qty] pairs, best first. Never nil.
func (s *side) depth(n int) [][2]int64 {
	out := make([][2]int64, 0, n)
	for i := len(s.levels) - 1; i >= 0 && len(out) < n; i-- {
		out = append(out, [2]int64{s.levels[i].px, s.levels[i].total})
	}
	return out
}

type book struct {
	bids side
	asks side
}

func newBook() *book {
	return &book{bids: side{isBid: true}, asks: side{isBid: false}}
}
