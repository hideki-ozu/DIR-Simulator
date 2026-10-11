package mailbox;
simple Mailbox {
 parameters:
  @class("dir.ipc.MailboxV1");
}
network Main {
 submodules:
  mailbox: mailbox.Mailbox;
}
