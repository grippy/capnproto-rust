// Copyright (c) 2015 Sandstorm Development Group, Inc. and contributors
// Licensed under the MIT License:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use capnp::message::ReaderOptions;
use gj::{ForkedPromise, Promise, PromiseFulfiller};

use std::cell::RefCell;
use std::rc::Rc;

pub type VatId = ::rpc_twoparty_capnp::Side;

pub struct IncomingMessage {
    message: ::capnp::message::Reader<::capnp_gj::serialize::OwnedSegments>,
}

impl IncomingMessage {
    pub fn new(message: ::capnp::message::Reader<::capnp_gj::serialize::OwnedSegments>) -> IncomingMessage {
        IncomingMessage { message: message }
    }
}

impl ::IncomingMessage for IncomingMessage {
    fn get_body<'a>(&'a self) -> ::capnp::Result<::capnp::any_pointer::Reader<'a>> {
        self.message.get_root()
    }
}

pub struct OutgoingMessage<U> where U: ::gj::io::AsyncWrite {
    message: ::capnp::message::Builder<::capnp::message::HeapAllocator>,
    write_queue: Rc<RefCell<::gj::Promise<U, ::capnp::Error>>>,
}

impl <U> ::OutgoingMessage for OutgoingMessage<U> where U: ::gj::io::AsyncWrite {
    fn get_body<'a>(&'a mut self) -> ::capnp::Result<::capnp::any_pointer::Builder<'a>> {
        self.message.get_root()
    }

    fn get_body_as_reader<'a>(&'a self) -> ::capnp::Result<::capnp::any_pointer::Reader<'a>> {
        self.message.get_root_as_reader()
    }

    fn send(self: Box<Self>)
            -> Promise<::capnp::message::Builder<::capnp::message::HeapAllocator>, ::capnp::Error>
    {
        let tmp = *self;
        let OutgoingMessage {message, write_queue} = tmp;
        let queue = ::std::mem::replace(&mut *write_queue.borrow_mut(), Promise::never_done());
        let (promise, fulfiller) = Promise::and_fulfiller();
        *write_queue.borrow_mut() = queue.then(move |s| {
            // DEBUG
            //println!("writing...");
            //use ::std::io::Write;
            //pry!(::capnp::serialize::write_message(&mut ::std::io::stdout(), &message));
            //::std::io::stdout().flush();
            ::capnp_gj::serialize::write_message(s, message).map(move |(s, m)| {
                fulfiller.fulfill(m);
                Ok(s)
            })
        });
        promise
    }
}

pub struct Connection<T, U> where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite {
    input_stream: Rc<RefCell<Option<T>>>,
    write_queue: Rc<RefCell<Promise<U, ::capnp::Error>>>,
    receive_options: ReaderOptions,
    on_disconnect_fulfiller: Option<PromiseFulfiller<(), ::capnp::Error>>,
}

impl <T, U> Connection<T, U> where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite {
    fn new(input_stream: T,
           output_stream: U,
           receive_options: ReaderOptions,
           on_disconnect_fulfiller: PromiseFulfiller<(), ::capnp::Error>,
           ) -> Connection<T, U> {
        Connection {
            input_stream: Rc::new(RefCell::new(Some(input_stream))),
            write_queue: Rc::new(RefCell::new(::gj::Promise::ok(output_stream))),
            receive_options: receive_options,
            on_disconnect_fulfiller: Some(on_disconnect_fulfiller),
        }
    }
}

impl <T, U> Drop for Connection<T, U> where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite {
    fn drop(&mut self) {
        let maybe_fulfiller = ::std::mem::replace(&mut self.on_disconnect_fulfiller, None);
        match maybe_fulfiller {
            Some(fulfiller) => {
                fulfiller.fulfill(());
            }
            None => unreachable!(),
        }
    }
}

impl <T, U> ::Connection<VatId> for Connection<T, U>
    where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite
{
    fn get_peer_vat_id(&self) -> VatId {
        unimplemented!()
    }

    fn new_outgoing_message(&mut self, _first_segment_word_size: u32) -> Box<::OutgoingMessage> {
        Box::new(OutgoingMessage {
            message: ::capnp::message::Builder::new_default(),
            write_queue: self.write_queue.clone()
        })
    }

    fn receive_incoming_message(&mut self) -> Promise<Option<Box<::IncomingMessage>>, ::capnp::Error> {
        self.receive_options;
        let maybe_input_stream = ::std::mem::replace(&mut *self.input_stream.borrow_mut(), None);
        let return_it_here = self.input_stream.clone();
        match maybe_input_stream {
            Some(s) => {
                ::capnp_gj::serialize::try_read_message(s, self.receive_options).map(move |(s, maybe_message)| {
                    *return_it_here.borrow_mut() = Some(s);
                    Ok(maybe_message.map(|message|
                                         Box::new(IncomingMessage::new(message)) as Box<::IncomingMessage>))
                })
            }
            None => panic!(),
        }
    }

    fn shutdown(&mut self) -> Promise<(), ::capnp::Error> {
        let write_queue = ::std::mem::replace(&mut *self.write_queue.borrow_mut(), Promise::never_done());
        write_queue.map(|_| Ok(()))
    }
}

pub struct VatNetwork<T, U> where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite {
    connection: Option<Connection<T,U>>,
    on_disconnect_promise: ForkedPromise<(), ::capnp::Error>,

}

impl <T, U> VatNetwork<T, U> where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite {
    pub fn new(input_stream: T, output_stream: U, receive_options: ReaderOptions) -> VatNetwork<T, U> {
        let (promise, fulfiller) = Promise::and_fulfiller();
        VatNetwork {
            connection: Some(Connection::new(input_stream, output_stream, receive_options, fulfiller)),
            on_disconnect_promise: promise.fork(),
        }
    }

    pub fn on_disconnect(&mut self) -> Promise<(), ::capnp::Error> {
        self.on_disconnect_promise.add_branch()
    }
}

impl <T, U> ::VatNetwork<VatId> for VatNetwork<T, U>
    where T: ::gj::io::AsyncRead, U: ::gj::io::AsyncWrite
{
    fn connect(&mut self, _host_id: VatId) -> Option<Box<::Connection<VatId>>> {
        let connection = ::std::mem::replace(&mut self.connection, None);
        connection.map(|c| Box::new(c) as Box<::Connection<VatId>>)
    }

    fn accept(&mut self) -> Promise<Box<::Connection<VatId>>, ::capnp::Error> {
        let connection = ::std::mem::replace(&mut self.connection, None);
        match connection {
            Some(c) => Promise::ok(Box::new(c) as Box<::Connection<VatId>>),
            None => Promise::never_done(),
        }
    }
}
